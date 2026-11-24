"""Failure isolation for the real Unix reference supervisor."""
import asyncio
import json
from pathlib import Path
import sys
import time

from reference_host import reference_host
from coglet_surface.surface import Surface

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import run_case


def test_disconnected_case_does_not_fill_control_backlog(tmp_path, monkeypatch):
    """A failed wire must not kill the consumer and cascade into connect EAGAIN."""
    config = json.loads((ROOT / 'spec/fixtures/surface/adapter-config.json').read_text())
    original = Surface.grant
    delayed = [False]

    def grant(surface, entries):
        if not delayed[0]:
            delayed[0] = True
            time.sleep(.4)  # Deliberately longer than unchanged 200-ms step 2.
        return original(surface, entries)

    monkeypatch.setattr(Surface, 'grant', grant)
    # A short private path also works on macOS.
    import tempfile
    with tempfile.TemporaryDirectory(prefix='coglet-') as directory:
        with reference_host(directory, config) as (wire, control):
            impl = dict(name='python-reference', transport='unix', socket=str(wire),
                peer_roles=['runtime'], fixtures={'surface-base': dict(
                    setup=[sys.executable, str(ROOT / 'sdk/surface-python/tests/provision_reference.py')],
                    env={'COGLET_FIXTURE_CONTROL': str(control)}),
                    'kill-at-100ms': dict(env={'COGLET_FIXTURE_KILL_MS': '100'})})
            case = json.loads((ROOT / 'conformance/surface/kill-latched.json').read_text())
            first = asyncio.run(run_case(impl, 'surface', case))
            assert 'step 2: TimeoutError' in first[3]
            # Previously two unserved setups fill listen(1), and the third
            # raises BlockingIOError(EAGAIN). Exercise every slot in that chain.
            next_case = json.loads((ROOT / 'conformance/surface/read-config.json').read_text())
            # This is supervisor liveness, not functional deadline qualification.
            # Negotiate only, with an infrastructure watchdog; frozen suite JSON
            # and its protocol expectations are exercised separately unchanged.
            next_case['steps'] = next_case['steps'][:4]
            for step in next_case['steps']:
                step['within_ms'] = 3000
            results = [asyncio.run(run_case(impl, 'surface', next_case)) for _ in range(3)]
            assert all(result[3] is None for result in results), results


def test_slow_negotiation_keeps_functional_deadlines(tmp_path):
    from test_conformance import registry_run, SDK
    wrapper = tmp_path / 'slow.py'
    wrapper.write_text(
        'import sys,time\n'
        'from coglet_surface.surface import Surface\n'
        'original=Surface.grant\n'
        'def slow(self, entries):\n'
        '    time.sleep(.4)\n'
        '    return original(self,entries)\n'
        'Surface.grant=slow\n'
        f'sys.path.insert(0,{str(SDK / "tests")!r})\n'
        'from clock_reference import main\n'
        'main()\n')
    results = asyncio.run(registry_run(tmp_path / 'delayed.xml', [sys.executable, str(wrapper)]))
    assert all(result[3] is None for result in results), results


def test_fixture_clock_refuses_rollback():
    from test_conformance import implementation
    from harness import Peer

    async def check():
        peer = Peer(implementation(), dict(__import__('os').environ, COGLET_CONFORMANCE_CASE='read-config'))
        try:
            await asyncio.wait_for(peer.start(), 5)
            await peer.advance_clock(100)
            # Bypass the harness's own rollback guard to exercise the adapter.
            peer.clock_writer.write((99).to_bytes(8, 'big'))
            await peer.clock_writer.drain()
            try:
                await asyncio.wait_for(peer.clock_reader.readexactly(8), 3)
            except asyncio.IncompleteReadError:
                pass
            else:
                raise AssertionError('retrograde advance acknowledged')
        finally:
            await peer.close()
    asyncio.run(check())


def test_partial_fixture_clock_fails_closed():
    from test_conformance import implementation
    from harness import Peer

    async def check():
        peer = Peer(implementation(), dict(__import__('os').environ, COGLET_CONFORMANCE_CASE='read-config'))
        try:
            await asyncio.wait_for(peer.start(), 5)
            peer.clock_writer.write(b'\x00' * 4)
            await peer.clock_writer.drain()
            peer.clock_writer.write_eof()
            assert await asyncio.wait_for(peer.process.wait(), 3) != 0
            await peer.stderr_task
            assert b'partial fixture clock' in peer.stderr
        finally:
            await peer.close()
    asyncio.run(check())
