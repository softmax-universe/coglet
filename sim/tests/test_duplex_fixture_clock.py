"""Same real speech adapter/ledger, with scheduler stalls beyond within_ms."""
import asyncio
import json
import copy
from pathlib import Path
import sys
import tempfile
import time
import socket
import pytest

from coglet_sim import host

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import run_case, validate_impl

CASE = json.loads((ROOT / 'conformance/speech/duplicate-call.json').read_text())
STALL = '''
import time
from coglet_sim import host
original = host.bind_session
def bind(*args, **kwargs):
    surface, device, configured = original(*args, **kwargs)
    def slow():
        time.sleep(.35)
        configured()
    return surface, device, slow
host.bind_session = bind
from coglet_sim.fixture_peer import main
main()
'''


def implementation(command=None):
    return dict(name='duplex-clock', suites=['speech'], transport='stdio',
                command=command or [sys.executable, '-m', 'coglet_sim.fixture_peer'],
                fixture_clock=True, fixture_scheduler=True,
                fixtures={'speech-handoff-duplicate-call': {'setup': []}})


def test_negotiation_stall_exceeds_unchanged_wall_deadline(monkeypatch):
    original = host.bind_session
    def bind(*args, **kwargs):
        surface, device, configured = original(*args, **kwargs)
        def slow():
            time.sleep(.35)
            configured()
        return surface, device, slow
    monkeypatch.setattr(host, 'bind_session', bind)
    with tempfile.TemporaryDirectory(prefix='duplex-stall-') as directory:
        with host.unix_supervisor(directory) as (wire, control):
            with socket.socket(socket.AF_UNIX) as peer:
                peer.connect(str(control))
                peer.sendall(b'{"case":"duplicate-call"}\n')
                assert peer.recv(32) == b'ready\n'
            impl = dict(name='wall-control', suites=['speech'], transport='unix',
                        socket=str(wire), fixtures={'speech-handoff-duplicate-call': {'setup': []}})
            result = asyncio.run(run_case(impl, 'speech', CASE))
    assert 'step 2: TimeoutError' in result[3], result


def test_slow_real_negotiation_keeps_logical_deadline():
    impl = implementation([sys.executable, '-c', STALL])
    validate_impl(impl)
    result = asyncio.run(run_case(impl, 'speech', CASE))
    assert result[3] is None, result


@pytest.mark.parametrize('name', ['barge-in-stop', 'duplicate-call',
                                 'heard-truncation', 'over-talk-duck'])
def test_slow_session_initialization_stays_in_startup_budget(name):
    # Startup has its existing 5 s budget; fixture steps have a 3 s liveness
    # bound. Delay real ledger/device initialization beyond the latter only.
    child = '''
import time
from coglet_sim import host
original = host.bind_session
def bind(*args, **kwargs):
    time.sleep(3.2)
    return original(*args, **kwargs)
host.bind_session = bind
from coglet_sim.fixture_peer import main
main()
'''
    case = json.loads((ROOT / f'conformance/speech/{name}.json').read_text())
    impl = implementation([sys.executable, '-c', child])
    impl['fixtures'] = {label: {'setup': []} for label in case['fixtures']}
    result = asyncio.run(run_case(impl, 'speech', case))
    assert result[3] is None, result


def test_missing_scheduled_frame_fails_at_original_logical_deadline():
    case = copy.deepcopy(CASE)
    case['steps'] = case['steps'][:5] + [{'expect': {}, 'within_ms': 1}]
    result = asyncio.run(run_case(implementation(), 'speech', case))
    assert 'logical fixture receive deadline' in result[3], result


def test_scheduled_silence_does_not_discard_unexpected_frame():
    case = copy.deepcopy(CASE)
    # Leave the real work_intent unread, then require silence.
    case['steps'] = case['steps'][:4] + [{'silence_ms': 25}]
    result = asyncio.run(run_case(implementation(), 'speech', case))
    assert 'unexpected bytes during silence' in result[3], result


@pytest.mark.parametrize('operation,target', [('next', -1), ('advance', True),
                                            ('advance', 2**63), ('invalid', 1)])
def test_scheduler_rejects_invalid_controls(tmp_path, monkeypatch, operation, target):
    from coglet_sim.fixture_peer import ScheduledSession
    monkeypatch.setenv('COGLET_CONFORMANCE_CASE', 'duplicate-call')
    session = ScheduledSession(tmp_path, lambda raw: None)
    try:
        with pytest.raises(ValueError, match='invalid scheduled clock control'):
            session.advance(operation, target)
        session.advance('advance', 10)
        with pytest.raises(ValueError, match='invalid scheduled clock control'):
            session.advance('advance', 9)
    finally:
        session.close()


def test_explicit_advance_runs_chronological_callbacks(tmp_path, monkeypatch):
    from coglet_sim.fixture_peer import ScheduledSession
    monkeypatch.setenv('COGLET_CONFORMANCE_CASE', 'duplicate-call')
    session = ScheduledSession(tmp_path, lambda raw: None)
    observed = []
    try:
        def first():
            observed.append(session.time)
            session.device.schedule(12, lambda: observed.append(session.time))
        session.device.schedule(10, first)
        session.advance('advance', 20_000)
        assert observed == [10_000, 12_000]
        assert session.time == 20_000
    finally:
        session.close()
