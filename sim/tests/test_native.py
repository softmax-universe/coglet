"""Real cogletd subprocess + Python SDK, enabled by sim/check-native.sh.

The ordinary Python job doesn't install a Rust toolchain. The Rust CI job runs
these named tests with all three built executable paths, never a mocked daemon.
"""
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import time

import pytest
from coglet_surface.codec import decode, decode_profile, encode_profile
from coglet_surface.transport import frame
from coglet_sim.surface import WorldSurface, runtime_binding
from coglet_sim.worlds import Cartpole, Gridbot

ROOT = Path(__file__).resolve().parents[2]
S = 'surface:' + 'a' * 64
C = 'coglet:' + 'b' * 64
WIRE = 'surface::surface-message'
TOOLS = ('COGLET_SANDBOX_DAEMON', 'COGLET_SANDBOX_PROOF', 'COGLET_SANDBOX_WASM')
pytestmark = pytest.mark.skipif(not all(os.environ.get(k) for k in TOOLS),
                              reason='native proof executes in Rust CI via sim/check-native.sh')


def setup(wasm, name):
    cases = json.loads((ROOT / 'conformance/abi/cases.json').read_text())
    result = next(c['steps'][0]['send'] for c in cases if c['name'] == 'no-effects')
    result['manifest'].update(id=C, subscriptions=[f'sim.{name}_state'],
        required_capabilities=[f'sim.{name}_' + ('move' if name == 'gridbot' else 'force')], exclusive_outputs=[])
    budgets = result['manifest']['budgets']
    budgets.update(compute_mode='wamr-2.4.3-fast-cumulative-v1', compute=100_000_000,
                   memory_pages=256, wall_time_us=2_000_000, host_bytes=1_000_000)
    result['initial_files']['/code/bin/step.wasm'] = wasm.hex()
    provenance = decode_profile(bytes.fromhex(result['initial_files']['/code/build/provenance.cbor']))
    from hashlib import sha256
    provenance['rewritten_sha256'] = sha256(wasm).hexdigest()
    result['initial_files']['/code/build/provenance.cbor'] = encode_profile(provenance).hex()
    result['native_schedule'] = dict(trace_id=result['inputs']['trace_id'], seed=7,
                                     runtime_bytes=16_777_216, stack_bytes=262_144)
    return result


def exact(sock, size):
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        assert chunk, 'cogletd disconnected'
        data.extend(chunk)
    return bytes(data)


def episode(tmp_path, kind):
    # pytest's default temp root can exceed AF_UNIX's 108-byte path bound.
    import tempfile
    with tempfile.TemporaryDirectory(prefix='m2-', dir=os.environ['TMPDIR']) as short:
        path = Path(short)
        assert len(str(path / 'sock/control').encode()) < 104
        world = kind(42, max_steps=16) if kind is Cartpole else kind(42)
        config = setup(Path(os.environ[TOOLS[2]]).read_bytes(), 'gridbot' if kind is Gridbot else 'cartpole')
        (path / 'setup.json').write_text(json.dumps(config))
        (path / 'binding.json').write_text(json.dumps(runtime_binding(S, [C])))
        stderr = (tmp_path / 'daemon.log').open('w+')
        child = subprocess.Popen([os.environ[TOOLS[0]], '--surface-service-socket',
            str(path / 'sock/control'), str(path / 'binding.json'), str(os.getuid()),
            str(path / 'state.wal'), str(path / 'setup.json')], stderr=stderr)
        surface = WorldSurface(S, [C], path / 'ledger', world)
        snapshots, actions = [world.state_hash()], []
        received_actions = {}
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            deadline = time.monotonic() + 10
            while True:
                assert child.poll() is None, 'daemon exited: ' + (tmp_path / 'daemon.log').read_text()
                try:
                    sock.connect(str(path / 'sock/control'))
                    break
                except (FileNotFoundError, ConnectionRefusedError):
                    assert time.monotonic() < deadline, 'daemon startup timeout'
                    time.sleep(.01)
            pid, uid, _ = struct.unpack('3i', sock.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            assert pid == child.pid and uid == os.getuid()
            sock.settimeout(5)
            sock.sendall(frame(surface.open()))
            initialized = False
            barrier_sent = False
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                raw = exact(sock, int.from_bytes(exact(sock, 4), 'big'))
                msg = decode(WIRE, raw)
                before_steps = world.steps
                surface.receive(raw)
                if surface.state == 'ready' and not initialized:
                    initialized = True
                    # Only the present world's operations are needed by its pilot.
                    # The manifest is narrowed to match before launching below.
                    surface.grant([dict(coglet=C, operations=list(surface.operations),
                                        resources=[], expires=surface.now() + 120_000_000)])
                    surface.publish(epoch=1)
                if msg['type'] == 'act':
                    aid = msg['body']['action']['id']
                    if aid in received_actions:
                        assert raw == received_actions[aid]
                        assert world.steps == before_steps, 'retry repeated the effect'
                    else:
                        assert world.steps == len(actions) + 1, f'action rejected: {msg}'
                        received_actions[aid] = raw
                        actions.append(msg['body']['action']['args']['direction'])
                        snapshots.append(world.state_hash())
                surface.tick()
                for out in surface.drain():
                    sock.sendall(frame(out))
                if world.done and not barrier_sent:
                    # A full health interval provides an ordered round-trip after
                    # the final state event, not a fabricated guest result.
                    sock.sendall(frame(surface._message('health', dict(state='ready',
                        clock=surface.now(), pending=0, correlation_id='9'*32))))
                    barrier_sent = True
                elif (barrier_sent and msg['type'] == 'health'
                      and msg['body']['correlation_id'] == '9'*32):
                    break
            assert world.done, 'episode failed to terminate'
            if kind is Gridbot:
                assert (world.x, world.y) == (world.goal_x, world.goal_y)
            sock.close()
            child.terminate()
            child.wait(timeout=5)
            proof = tmp_path / 'proof.json'
            subprocess.run([os.environ[TOOLS[1]], str(path/'setup.json'), str(path/'state.wal'),
                            str(proof)], check=True, timeout=90)
            report = json.loads(proof.read_text())
            assert report['independent_guest_reexecution']
            assert report['changed_seed_rejected'] and report['changed_guest_rejected']
            emitted = [a['args']['direction'] for r in report['records'] for a in r['actions']]
            assert emitted == actions
            assert len(report['state_hashes']) >= len(actions)
            assert any(e['kind'] == surface.event_name and e['payload'].get('done')
                       for r in report['records'] for e in r['events']), 'terminal observation not executed'
            replayed = kind(42, max_steps=16) if kind is Cartpole else kind(42)
            assert replayed.state_hash() == snapshots[0]
            for action, expected in zip(actions, snapshots[1:], strict=True):
                replayed.step(action)
                assert replayed.state_hash() == expected
            wrong_seed = kind(43, max_steps=16) if kind is Cartpole else kind(43)
            assert wrong_seed.state_hash() != snapshots[0]
            wrong_action = kind(42, max_steps=16) if kind is Cartpole else kind(42)
            opposite = dict(east='west', west='east', north='south', south='north', left='right', right='left')
            wrong_action.step(opposite[actions[0]])
            assert wrong_action.state_hash() != snapshots[1]
            print(f'{kind.__name__}: {len(actions)} actions; {len(report["state_hashes"])} live/replay guest hashes equal')
        finally:
            sock.close()
            surface.close()
            if child.poll() is None:
                child.kill()
                child.wait(timeout=5)
            stderr.close()


def test_gridbot_pilot_reaches_goal_live_and_replayed(tmp_path):
    episode(tmp_path, Gridbot)


def test_cartpole_episode_live_and_replayed(tmp_path):
    episode(tmp_path, Cartpole)
