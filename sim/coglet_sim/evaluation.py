"""Fresh-ledger native evaluation; fixture provenance, NOT signed gate authority."""
from dataclasses import dataclass
from hashlib import sha256
import json
import os
from pathlib import Path
import socket
import select
import struct
import subprocess
import tempfile
import time

from coglet_surface.codec import decode, decode_profile, encode_profile
from coglet_surface.transport import Frames, Output
from coglet_sim.surface import WorldSurface, runtime_binding
from coglet_sim.worlds import Cartpole, Gridbot
from coglet_sim._native_diagnostics import retain_failure

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


class _EvaluatorLink:
    """One socket owner: service timers even during partial stream I/O.

    No peer authority is refreshed here; only Surface.receive accepts messages.
    select/clock seams are private deterministic test controls, not policy knobs.
    """
    def __init__(self, sock, surface, *, deadline=None, clock=None, select_fn=None):
        self.sock, self.surface = sock, surface
        self.deadline = deadline
        self.clock = clock or time.monotonic
        self.select = select_fn or select.select
        self.parser, self.pending = Frames(), Output()
        self.head_deadline = None
        self.last_sent = None
        self.closed = False
        sock.setblocking(False)

    def enqueue(self, raw):
        assert not self.closed
        self.pending.append(raw, self.surface.revocation)

    def service(self):
        assert not self.closed
        self.surface.tick()
        self.pending.revoke(self.surface.revocation, killed=bool(self.surface._get('killed')))
        for raw in self.surface.drain():
            self.enqueue(raw)

    def _turn(self, read_deadline=None):
        self.service()
        now = self.clock()
        if self.deadline is not None and now >= self.deadline:
            raise TimeoutError('episode deadline exceeded')
        if read_deadline is not None and now >= read_deadline:
            raise TimeoutError('native receive timeout')
        # Bind the sendall-equivalent budget to the exact head frame, including
        # priority insertion/revocation. Partial frames cannot be reprioritized.
        head = self.pending.frames[0] if self.pending else None
        if head is not getattr(self, '_head', None):
            self._head = head
            self.head_deadline = now + 5 if head is not None else None
        if self.head_deadline is not None and now >= self.head_deadline:
            raise TimeoutError('native send timeout')
        limits = [now + .01]
        limits.extend(d for d in (self.deadline, read_deadline, self.head_deadline) if d is not None)
        readable, writable, _ = self.select(
            [self.sock] if read_deadline is not None else [],
            [self.sock] if self.pending else [], [], max(0, min(limits) - now))
        # Return to service before I/O if the selector crossed a deadline. This
        # also prevents a continuous ready stream from bypassing timer checks.
        now = self.clock()
        if any(now >= d for d in (self.deadline, read_deadline, self.head_deadline) if d is not None):
            return None
        if writable and self.pending:
            try:
                sent = self.sock.send(self.pending.peek())
            except BlockingIOError:
                sent = 0
            if sent:
                self.pending.consume(sent)
                if self.pending.offset == 0:
                    self.last_sent = self.clock()
        if readable:
            needed = (4 if self.parser.size is None else self.parser.size) - len(self.parser.buffer)
            try:
                data = self.sock.recv(needed)
            except BlockingIOError:
                return None
            if not data:
                self.parser.eof()
                raise AssertionError('cogletd disconnected')
            return data
        return None

    def receive(self):
        progress_deadline = self.clock() + 5
        while True:
            data = self._turn(progress_deadline)
            if data is not None:
                progress_deadline = self.clock() + 5
                frames = self.parser.feed(data)
                if frames:
                    assert len(frames) == 1
                    return frames[0]

    def flush(self):
        # Service once even if currently empty, as the old post-frame path did.
        self.service()
        while self.pending:
            self._turn()

    def close(self):
        # No sends on failure/cleanup, no surviving writer or owned socket close.
        self.closed = True
        self.parser, self.pending = Frames(), Output()


def evaluate(tmp_path, tools, wasm, *, world, seed, max_steps=16, minimum_actions=1, coglet=C, archive=None):
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
    # Hosted coordinator credentials have no role in local native acquisition.
    native_env = {k: v for k, v in os.environ.items()
                  if k not in ('CAOS_IROH_TICKET', 'UNFORGED_CAOS_URL')}

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
        link = None
        started = time.monotonic()
        deadline = None
        phase = 'startup'
        native_failure = None
        diagnostic = {}
        initialized = barrier_sent = False
        last_sent_at = last_received_at = last_message = None
        try:
            child = subprocess.Popen([str(tools.daemon), '--surface-service-socket',
                str(path / 'sock/control'), str(path / 'binding.json'), str(os.getuid()),
                str(path / 'state.wal'), str(path / 'setup.json')], stderr=stderr, env=native_env)
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
            link = _EvaluatorLink(sock, surface)
            link.enqueue(surface.open())
            link.flush()
            initialized = False
            barrier_sent = False
            deadline = time.monotonic() + 90
            link.deadline = deadline
            phase = 'loop'
            while time.monotonic() < deadline:
                raw = link.receive()
                last_received_at = time.monotonic() - started
                msg = decode(WIRE, raw)
                last_message = msg['type']
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
                link.service()
                if world.done and not barrier_sent:
                    link.enqueue(surface._message('health', dict(state='ready',
                        clock=surface.now(), pending=0, correlation_id='9'*32)))
                    barrier_sent = True
                elif (barrier_sent and msg['type'] == 'health'
                      and msg['body']['correlation_id'] == '9'*32):
                    link.flush()
                    break
                link.flush()
            else:
                raise TimeoutError('episode deadline exceeded')
            assert world.done, 'episode failed to terminate'
            phase = 'post-loop-terminate'
            sock.close()
            child.terminate()
            child.wait(timeout=5)
            proof = tmp_path / 'proof.json'
            phase = 'proof'
            proof_command = [str(tools.proof), str(path/'setup.json'), str(path/'state.wal'), str(proof)]
            if archive is not None:
                proof_command.append(str(Path(archive).resolve()))
            subprocess.run(proof_command, check=True, timeout=90, env=native_env)
            report = json.loads(proof.read_text())
            phase = 'replay'
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
        except Exception as error:
            native_failure = error
            if link is not None and link.last_sent is not None:
                last_sent_at = link.last_sent - started
            # Diagnostic failures never replace the original native exception.
            try:
                diagnostic = dict(phase=phase, elapsed=time.monotonic() - started,
                    exception=type(error).__name__, errno=getattr(error, 'errno', None),
                    daemon_pid=child.pid if child else None,
                    observed_returncode=child.poll() if child else None,
                    status_observation='at exception; may race daemon error logging',
                    actions=len(actions), steps=world.steps, surface_state=surface.state,
                    initialized=initialized, barrier_sent=barrier_sent,
                    last_sent_at=last_sent_at, last_received_at=last_received_at,
                    last_message=last_message,
                    deadline_remaining=deadline-time.monotonic() if deadline is not None else None)
                retain_failure(error, tmp_path, diagnostic)
            except Exception:
                pass
            raise
        finally:
            if link is not None:
                link.close()
            sock.close()
            surface.close()
            supervisor_killed = False
            if child is not None and child.poll() is None:
                supervisor_killed = True
                child.kill()
                child.wait(timeout=5)
            stderr.close()
            if native_failure is not None:
                try:
                    diagnostic['cleanup'] = dict(supervisor_killed=supervisor_killed,
                        returncode=child.returncode if child else None,
                        status_observation='after existing supervisor cleanup')
                    retain_failure(native_failure, tmp_path, diagnostic)
                except Exception:
                    pass
