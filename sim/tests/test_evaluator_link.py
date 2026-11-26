"""Deterministic characterization of the evaluator-only link (review cases 2–7).

Not a claim of genuine pre-implementation RED or historical failure causality.
Real SDK controls use negotiated WorldSurface; only explicit deadline-isolation
cases use MinimalSurface. All stream bytes pass through actual socketpairs.
"""
from contextlib import contextmanager
import select
import socket

import pytest

from coglet_surface.codec import decode_profile, encode, encode_profile
from coglet_surface.surface import MAX_FRAME, ProtocolError
from coglet_surface.transport import Frames, frame
from coglet_sim.evaluation import C, S, WIRE, _EvaluatorLink
from coglet_sim.surface import WorldSurface
from coglet_sim.worlds import Gridbot


class Clock:
    def __init__(self):
        self.us = 0

    def __call__(self):
        return self.us / 1_000_000

    def sdk(self):
        return self.us


class ScriptSelect:
    """Finite, non-sleeping scheduler; readiness itself is the real OS result."""
    def __init__(self, clock, peer):
        self.clock, self.peer = clock, peer
        self.calls = 0
        self.hook = lambda scheduler: None
        self.received = bytearray()
        self.waits = []

    def collect(self):
        while True:
            try:
                data = self.peer.recv(65536)
            except BlockingIOError:
                break
            if not data:
                break
            self.received.extend(data)

    def __call__(self, reads, writes, errors, timeout):
        self.calls += 1
        assert self.calls <= 2000, 'finite selector cap exhausted'
        assert 0 <= timeout <= .010001
        self.waits.append(timeout)
        self.clock.us += round(timeout * 1_000_000)
        self.collect()
        self.hook(self)
        return select.select(reads, writes, errors, 0)


class SendShim:
    """Short/blocked writes are imposed, but successful bytes use a real socket."""
    def __init__(self, sock):
        self.sock = sock
        self.calls = 0
        self.limit = None
        self.block = lambda call: False
        self.sent = bytearray()
        self.recv_calls = 0

    def setblocking(self, value):
        self.sock.setblocking(value)

    def fileno(self):
        return self.sock.fileno()

    def send(self, data):
        self.calls += 1
        if self.block(self.calls):
            raise BlockingIOError()
        count = self.sock.send(data if self.limit is None else data[:self.limit])
        self.sent.extend(data[:count])
        return count

    def recv(self, size):
        self.recv_calls += 1
        return self.sock.recv(size)


class MinimalSurface:
    """No SDK peer timer: isolates five-second I/O and absolute episode limits."""
    revocation = 0

    def __init__(self):
        self.ticks = 0

    def tick(self):
        self.ticks += 1

    def drain(self):
        return []

    def _get(self, name):
        assert name == 'killed'
        return False


@contextmanager
def rig(tmp_path, *, minimal=False, deadline=None):
    clock = Clock()
    surface = MinimalSurface() if minimal else WorldSurface(
        S, [C], tmp_path / 'ledger', Gridbot(42), clock=clock.sdk)
    if not minimal:
        surface.open('1' * 32)
        surface.receive(encode(WIRE, dict(type='hello_reply', body=dict(
            selection=dict(version='0.1'), correlation_id='1' * 32))))
        assert surface.state == 'ready'
        surface.grant([dict(coglet=C, operations=list(surface.operations),
                            resources=[], expires=120_000_000)])
        surface.drain()
    local, peer = socket.socketpair()
    peer.setblocking(False)
    shim = SendShim(local)
    scheduler = ScriptSelect(clock, peer)
    link = _EvaluatorLink(shim, surface, deadline=deadline, clock=clock,
                          select_fn=scheduler)
    try:
        yield link, surface, clock, scheduler, shim, peer
    finally:
        link.close()
        local.close()
        peer.close()
        if not minimal:
            surface.close()


def health(clock=0, cid='2'):
    return encode(WIRE, dict(type='health', body=dict(
        state='ready', clock=clock, pending=0, correlation_id=cid * 32)))


def outputs(scheduler):
    scheduler.collect()
    parser = Frames()
    result = parser.feed(scheduler.received)
    parser.eof()
    return result


def test_evaluator_partial_header_and_body_service_ticks(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        raw = health(2_200_000)
        wire = frame(raw)
        stages = [(10_000, wire[:2]), (1_100_000, wire[2:7]),
                  (2_200_000, wire[7:])]
        observations = []

        def step(s):
            if stages and clock.us >= stages[0][0]:
                _, data = stages.pop(0)
                if clock.us >= 1_100_000:
                    observations.append((clock.us, len(s.received), surface.last_received))
                    assert s.received, 'health must reach peer before fragment completion'
                assert peer.send(data) == len(data)

        schedule.hook = step
        assert link.receive() == raw
        assert not stages
        assert len(observations) == 2
        assert all(last == 0 for _, _, last in observations)
        assert surface.last_received == 0  # receive() never interprets application frames
        assert link.parser.size is None and not link.parser.buffer
        surface.receive(raw)
        assert surface.last_received == 2_200_000
        messages = [decode_profile(raw) for raw in outputs(schedule)]
        assert len(messages) == 2
        assert [m['type'] for m in messages] == ['health', 'health']
        assert schedule.calls <= 225


@pytest.mark.parametrize('partial', [False, True], ids=['silent', 'partial-bytes'])
def test_evaluator_partial_bytes_and_local_sends_do_not_refresh_peer(tmp_path, partial):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        # A deliberately unfinished large frame drips bytes while local health flows.
        sent = 0

        def step(s):
            nonlocal sent
            if partial and s.calls % 20 == 1:
                data = (10000).to_bytes(4, 'big') if sent == 0 else b'x'
                assert peer.send(data) == len(data)
                sent += len(data)

        schedule.hook = step
        generation = surface.revocation
        with pytest.raises(ProtocolError, match='peer heartbeat timeout'):
            link.receive()
        assert clock.us == 3_000_000
        assert surface.last_received == 0
        assert surface.state == 'closed' and surface.grants == []
        assert surface.revocation == generation + 1
        assert shim.sent, 'local output must actually have been transmitted'
        assert len(outputs(schedule)) == 2
        assert bool(sent) == partial
        assert schedule.calls == 300


def test_evaluator_partial_write_fifo_and_bound(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        raws = [health(cid=str(n)) for n in range(3, 6)]
        shim.limit = 7
        shim.block = lambda call: call in (1, 3, 8)
        for raw in raws:
            link.enqueue(raw)
        link.flush()
        assert outputs(schedule) == raws
        assert bytes(shim.sent) == b''.join(map(frame, raws))
        assert shim.calls > len(raws) and clock.us > 0
        assert surface.last_received == 0
        assert link.last_sent == clock()
        for _ in range(128):
            link.enqueue(raws[0])
        with pytest.raises(ProtocolError, match='transport queue overflow'):
            link.enqueue(raws[0])
        assert len(link.pending.frames) == 128
        assert schedule.calls < 100


def private_event():
    # Output classification tests deliberately avoid unrelated event validation.
    return encode_profile(dict(type='event', body=dict(kind='sim.gridbot_state')))


def public_outcome():
    return encode_profile(dict(type='event', body=dict(kind='action.outcome')))


def test_evaluator_output_revocation_removes_private_retains_public(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        link.enqueue(private_event())
        link.enqueue(public_outcome())
        surface.grant([])  # actual SDK generation transition, not a fake counter
        sdk_output = surface.drain()
        for raw in sdk_output:
            link.enqueue(raw)
        link.flush()
        assert outputs(schedule) == [public_outcome(), *sdk_output]


def test_evaluator_output_revocation_partial_fails_closed(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        shim.limit = 2
        link.enqueue(private_event())
        link._turn()
        assert link.pending.offset == 2
        before = bytes(shim.sent)
        surface.grant([])
        with pytest.raises(ProtocolError, match='revoked partial private frame'):
            link.flush()
        assert bytes(shim.sent) == before
        assert shim.calls == 1


@pytest.mark.parametrize('mode', ['receive', 'flush'])
def test_evaluator_absolute_episode_deadline_under_io_activity(tmp_path, mode):
    with rig(tmp_path, deadline=2.5) as (link, surface, clock, schedule, shim, peer):
        shim.limit = 1
        # Real valid SDK peer health is explicitly handled by the application seam;
        # partial stream bytes and successful local sends alone cannot do this.
        def step(s):
            if s.calls % 50 == 0:
                surface.receive(health(clock.us))
            if mode == 'receive':
                data = (MAX_FRAME).to_bytes(4, 'big') if s.calls == 1 else b'x'
                assert peer.send(data) == len(data)
        schedule.hook = step
        for _ in range(20):
            link.enqueue(health())
        with pytest.raises(TimeoutError, match='episode deadline exceeded'):
            getattr(link, mode)()
        assert clock.us == 2_500_000
        assert surface.state == 'ready'
        assert surface.last_received >= 2_000_000
        assert shim.sent
        assert schedule.calls == 250


def test_evaluator_receive_five_seconds_without_bytes_minimal_surface(tmp_path):
    with rig(tmp_path, minimal=True) as (link, surface, clock, schedule, shim, peer):
        with pytest.raises(TimeoutError, match='native receive timeout'):
            link.receive()
        assert clock.us == 5_000_000 and schedule.calls == 500
        assert surface.ticks >= 500


def test_evaluator_receive_progress_resets_read_not_whole_frame_minimal_surface(tmp_path):
    with rig(tmp_path, minimal=True) as (link, surface, clock, schedule, shim, peer):
        raw = health()
        wire = frame(raw)
        pieces = [(4_900_000, wire[:2]), (9_800_000, wire[2:5]),
                  (14_700_000, wire[5:])]
        def step(s):
            if pieces and clock.us >= pieces[0][0]:
                _, data = pieces.pop(0)
                assert peer.send(data) == len(data)
        schedule.hook = step
        assert link.receive() == raw
        assert clock.us == 14_700_000
        assert not pieces and schedule.calls == 1470


def test_evaluator_read_progress_timeout_after_partial_minimal_surface(tmp_path):
    with rig(tmp_path, minimal=True) as (link, surface, clock, schedule, shim, peer):
        def step(s):
            if s.calls == 490:
                assert peer.send(b'\x00') == 1
        schedule.hook = step
        with pytest.raises(TimeoutError, match='native receive timeout'):
            link.receive()
        assert clock.us == 9_900_000
        assert schedule.calls == 990


@pytest.mark.parametrize('progress', [False, True], ids=['blocked', 'short-progress'])
def test_evaluator_head_send_total_five_seconds_with_real_peer_health(tmp_path, progress):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        # Long private frame cannot finish even with a byte of progress per turn.
        raw = encode_profile(dict(type='event', body=dict(kind='sim.gridbot_state',
                                                         padding='x' * 1000)))
        link.enqueue(raw)
        shim.limit = 1
        shim.block = lambda call: not progress or call % 2 == 0
        def step(s):
            if s.calls % 50 == 0:
                surface.receive(health(clock.us))
        schedule.hook = step
        with pytest.raises(TimeoutError, match='native send timeout'):
            link.flush()
        assert clock.us == 5_000_000
        assert surface.state == 'ready' and surface.last_received >= 4_500_000
        assert bool(shim.sent) == progress
        assert schedule.calls == 500


def test_evaluator_new_head_gets_new_send_budget_minimal_surface(tmp_path):
    with rig(tmp_path, minimal=True) as (link, surface, clock, schedule, shim, peer):
        raws = [health(cid='3'), health(cid='4')]
        for raw in raws:
            link.enqueue(raw)
        shim.block = lambda call: schedule.calls not in (490, 980)
        link.flush()
        assert outputs(schedule) == raws
        assert clock.us == 9_800_000 and schedule.calls == 980


@pytest.mark.parametrize('size', [0, MAX_FRAME + 1, 2**32 - 1])
def test_evaluator_frame_bounds_rejected_before_payload(tmp_path, size):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        assert peer.send(size.to_bytes(4, 'big')) == 4
        with pytest.raises(ProtocolError, match='invalid frame length'):
            link.receive()
        assert not link.parser.buffer
        assert shim.recv_calls == 1 and schedule.calls == 1


@pytest.mark.parametrize('prefix', [b'', b'\x00', b'\x00\x00\x00',
                                  b'\x00\x00\x00\x03', b'\x00\x00\x00\x03a'])
def test_evaluator_frame_eof_clean_and_partial(tmp_path, prefix):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        if prefix:
            assert peer.send(prefix) == len(prefix)
        peer.shutdown(socket.SHUT_WR)
        error, match = (ProtocolError, 'EOF mid-frame') if prefix else (
            AssertionError, 'cogletd disconnected')
        with pytest.raises(error, match=match):
            link.receive()
        assert schedule.calls <= 3


def test_evaluator_maximum_length_and_consecutive_frames_once(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        # Transport returns opaque payloads; semantic CBOR acceptance is application-owned.
        raws = [b'x' * MAX_FRAME, health(cid='3'), health(cid='4')]
        wire = b''.join(map(frame, raws))
        offset = 0
        def step(s):
            nonlocal offset
            if offset < len(wire):
                count = peer.send(wire[offset:offset + 16384])
                offset += count
        schedule.hook = step
        assert [link.receive() for _ in raws] == raws
        assert offset == len(wire)
        assert link.parser.size is None and not link.parser.buffer
        assert not select.select([shim], [], [], 0)[0]
        assert schedule.calls < 100


def test_evaluator_kill_priority_never_interrupts_partial_public_head(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        head, tail = public_outcome(), health(cid='7')
        link.enqueue(head)
        link.enqueue(tail)
        shim.limit = 2
        link._turn()
        assert link.pending.offset == 2
        # Keep the priority oracle below the periodic one-second health boundary;
        # only the initial short write is needed to establish a partial head.
        shim.limit = None
        surface.kill('test stop')
        link.flush()
        actual = outputs(schedule)
        assert actual[0] == head and actual[-1] == tail
        assert len(actual) == 4
        assert decode_profile(actual[1])['type'] == 'kill'
        stopped = decode_profile(actual[2])
        assert stopped['type'] == 'health' and stopped['body']['state'] == 'stopped'
        assert bytes(shim.sent) == b''.join(map(frame, actual))


@pytest.mark.parametrize('operation', ['receive', 'flush'])
def test_evaluator_selector_clamps_episode_and_no_io_at_expiry_minimal_surface(
        tmp_path, operation):
    with rig(tmp_path, minimal=True, deadline=.025) as (
            link, surface, clock, schedule, shim, peer):
        link.enqueue(health())
        shim.block = lambda call: True
        def step(s):
            if s.calls == 3:
                assert peer.send(frame(health())) > 0
                shim.block = lambda call: False
        schedule.hook = step
        with pytest.raises(TimeoutError, match='episode deadline exceeded'):
            getattr(link, operation)()
        assert schedule.waits == pytest.approx([.01, .01, .005])
        assert clock.us == 25_000
        assert not shim.sent and shim.recv_calls == 0
        assert shim.calls == 2  # no third send attempt on deadline boundary


def test_evaluator_receive_bytes_exactly_at_read_deadline_are_too_late_minimal_surface(tmp_path):
    with rig(tmp_path, minimal=True) as (link, surface, clock, schedule, shim, peer):
        def step(s):
            if s.calls == 500:
                assert peer.send(frame(health())) > 0
        schedule.hook = step
        with pytest.raises(TimeoutError, match='native receive timeout'):
            link.receive()
        assert clock.us == 5_000_000 and shim.recv_calls == 0


def test_evaluator_close_discards_without_sending_or_owning_socket(tmp_path):
    with rig(tmp_path) as (link, surface, clock, schedule, shim, peer):
        link.enqueue(health())
        link.parser.feed(b'\x00')
        link.close()
        assert not link.pending and not link.parser.buffer
        assert shim.calls == 0 and schedule.calls == 0
        assert shim.fileno() >= 0 and surface.state == 'ready'
        with pytest.raises(AssertionError):
            link.enqueue(health())
        with pytest.raises(AssertionError):
            link.service()
