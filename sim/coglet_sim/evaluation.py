"""Fresh-ledger native evaluation; fixture provenance, NOT signed gate authority."""
from dataclasses import dataclass
from hashlib import sha256
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time

from coglet_surface.codec import decode, decode_profile, encode_profile
from coglet_surface.transport import frame
from coglet_sim.surface import WorldSurface, runtime_binding
from coglet_sim.worlds import Cartpole, Gridbot

ROOT = Path(__file__).resolve().parents[2]
S = 'surface:' + 'a' * 64
C = 'coglet:' + 'b' * 64
WIRE = 'surface::surface-message'


@dataclass(frozen=True)
class NativeTools:
    daemon: Path
    proof: Path

    def __post_init__(self):
        for path in (self.daemon, self.proof):
            if not path.is_absolute() or not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError('native tool must be an existing absolute executable')


def setup(wasm, name, seed=7, coglet=C):
    cases = json.loads((ROOT / 'conformance/abi/cases.json').read_text())
    result = next(c['steps'][0]['send'] for c in cases if c['name'] == 'no-effects')
    result['manifest'].update(id=coglet, subscriptions=[f'sim.{name}_state'],
        required_capabilities=[f'sim.{name}_' + ('move' if name == 'gridbot' else 'force')], exclusive_outputs=[])
    result['manifest']['budgets'].update(compute_mode='wamr-2.4.3-fast-cumulative-v1',
        compute=100_000_000, memory_pages=256, wall_time_us=2_000_000, host_bytes=1_000_000)
    result['initial_files']['/code/bin/step.wasm'] = wasm.hex()
    provenance = decode_profile(bytes.fromhex(result['initial_files']['/code/build/provenance.cbor']))
    provenance['rewritten_sha256'] = sha256(wasm).hexdigest()
    result['initial_files']['/code/build/provenance.cbor'] = encode_profile(provenance).hex()
    result['native_schedule'] = dict(trace_id=result['inputs']['trace_id'], seed=seed,
                                     runtime_bytes=16_777_216, stack_bytes=262_144)
    return result


def exact(sock, size):
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        assert chunk, 'cogletd disconnected'
        data.extend(chunk)
    return bytes(data)


def evaluate(tmp_path, tools, wasm, *, world, seed, max_steps=16, minimum_actions=1, coglet=C):
    """Execute exact rewritten bytes, independently replay, check behavior.

    Caller pins tools, world and assertions. Returned data is unsigned, not a
    transferable policy report. Infrastructure/protocol/replay failures raise;
    behavioral assertion failures return passed=False. No live deployment occurs.
    """
    # Replay checks inherited from M2 use assertions; never silently omit them.
    if not __debug__:
        raise RuntimeError('optimized interpreter unsupported')
    if world not in ('gridbot', 'cartpole'):
        raise ValueError('unknown world')
    if not isinstance(wasm, bytes) or not wasm:
        raise ValueError('nonempty rewritten wasm bytes required')
    if type(minimum_actions) is not int or minimum_actions < 1:
        raise ValueError('positive minimum_actions required')
    if type(seed) is not int or not 0 <= seed < 2**64:
        raise ValueError('u64 seed required')
    if type(max_steps) is not int or not 1 <= max_steps <= 1024:
        raise ValueError('bounded max_steps required')
    name = world
    kind = Gridbot if name == 'gridbot' else Cartpole

    make_world = lambda s: kind(s, max_steps=max_steps)
    with tempfile.TemporaryDirectory(prefix='eval-', dir=os.environ['TMPDIR']) as short:
        path = Path(short)
        assert len(str(path / 'sock/control').encode()) < 104
        world = make_world(seed)
        config = setup(wasm, name, seed, coglet)
        (path / 'setup.json').write_text(json.dumps(config))
        (path / 'binding.json').write_text(json.dumps(runtime_binding(S, [coglet])))
        surface = WorldSurface(S, [coglet], path / 'ledger', world)
        stderr = (tmp_path / 'daemon.log').open('w+')
        snapshots, actions = [world.state_hash()], []
        received_actions = {}
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        child = None
        try:
            child = subprocess.Popen([str(tools.daemon), '--surface-service-socket',
                str(path / 'sock/control'), str(path / 'binding.json'), str(os.getuid()),
                str(path / 'state.wal'), str(path / 'setup.json')], stderr=stderr)
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
                    surface.grant([dict(coglet=coglet, operations=list(surface.operations),
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
                    sock.sendall(frame(surface._message('health', dict(state='ready',
                        clock=surface.now(), pending=0, correlation_id='9'*32))))
                    barrier_sent = True
                elif (barrier_sent and msg['type'] == 'health'
                      and msg['body']['correlation_id'] == '9'*32):
                    break
            assert world.done, 'episode failed to terminate'
            sock.close()
            child.terminate()
            child.wait(timeout=5)
            proof = tmp_path / 'proof.json'
            subprocess.run([str(tools.proof), str(path/'setup.json'), str(path/'state.wal'),
                            str(proof)], check=True, timeout=90)
            report = json.loads(proof.read_text())
            assert report['independent_guest_reexecution']
            assert report['changed_seed_rejected'] and report['changed_guest_rejected']
            emitted = [a['args']['direction'] for r in report['records'] for a in r['actions']]
            assert emitted == actions
            assert len(report['state_hashes']) >= len(actions)
            assert any(e['kind'] == surface.event_name and e['payload'].get('done')
                       for r in report['records'] for e in r['events']), 'terminal observation not executed'
            replayed = make_world(seed)
            assert replayed.state_hash() == snapshots[0]
            for action, expected in zip(actions, snapshots[1:], strict=True):
                replayed.step(action)
                assert replayed.state_hash() == expected
            assert make_world(seed ^ 1).state_hash() != snapshots[0]
            wrong_action = make_world(seed)
            opposite = dict(east='west', west='east', north='south', south='north', left='right', right='left')
            wrong_action.step(opposite[actions[0]])
            assert wrong_action.state_hash() != snapshots[1]
            failure = None
            if kind is Gridbot and (world.x, world.y) != (world.goal_x, world.goal_y):
                failure = 'goal_not_reached'
            elif kind is Cartpole and world.steps < max_steps:
                failure = 'episode_terminated_early'
            elif len(actions) < minimum_actions:
                failure = 'minimum_actions'
            return dict(world=name, seed=seed, max_steps=max_steps, minimum_actions=minimum_actions, coglet=coglet,
                        rewritten_sha256=sha256(wasm).hexdigest(), action_count=len(actions),
                        world_hashes=snapshots, runtime_state_hashes=report['state_hashes'],
                        independent_guest_reexecution=True, passed=failure is None, failure=failure)
        finally:
            sock.close()
            surface.close()
            if child is not None and child.poll() is None:
                child.kill()
                child.wait(timeout=5)
            stderr.close()
