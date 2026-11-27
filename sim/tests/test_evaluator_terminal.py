"""Reqs 8/9: evaluate control-path tests, NOT daemon/native replay proof.

Real SDK world, link, socketpair bytes and a real killable/reaped child. Injected
connect/credential/startup seams do not authenticate that child as a daemon.
Proof always raises before producing a report; no synthetic replay authority.
"""
import errno
import json
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace

import pytest

from coglet_surface.codec import decode, encode
from coglet_surface.surface import ProtocolError
from coglet_surface.transport import Frames, frame
from coglet_sim import evaluation as ev
from test_evaluator_link import Clock, SendShim


class BeforeProof(RuntimeError):
    pass


class TerminalHarness:
    """Finite single-owner scripted peer; no threads, sleeps or native claims."""
    def __init__(self, tmp_path, monkeypatch, mode):
        self.mode = mode
        self.clock = Clock()
        self.local, self.peer = socket.socketpair()
        self.peer.setblocking(False)
        self.shim = SendShim(self.local)
        self.events = []
        self.messages = []
        self.parser = Frames()
        self.calls = self.receives = 0
        self.terminal_received = False
        self.queued_terminal_outcome = False
        self.proof_calls = 0
        self.original_error = BrokenPipeError(errno.EPIPE, 'injected send failure')
        self.child = None
        self.surface = None
        self.link = None
        self.action = None
        self.pending_at_terminal = False
        self.last_health = 0
        real_popen = subprocess.Popen
        real_surface = ev.WorldSurface
        real_link = ev._EvaluatorLink

        def popen(*args, **kwargs):
            # A real disposable child, explicitly NOT a protocol/native daemon.
            self.child = real_popen([sys.executable, '-c',
                                    'import sys; sys.stdin.buffer.read()'],
                                   stdin=subprocess.PIPE, stderr=kwargs['stderr'])
            return self.child

        def surface(*args, **kwargs):
            self.surface = real_surface(*args, **kwargs, clock=self.clock.sdk)
            receive = self.surface.receive
            close = self.surface.close

            def receive_control(raw):
                message = decode(ev.WIRE, raw)
                receive(raw)
                if (message['type'] == 'health' and
                        message['body']['correlation_id'] == '9' * 32):
                    self.terminal_received = True
                    self.events.append('terminal-received')
                    # Explicit control seam: durable SDK retry emits a real public
                    # outcome while evaluating the final response. No second effect.
                    receive(self.action)
                    self.queued_terminal_outcome = True
                    if self.mode == 'blocked-final':
                        self.shim.block = lambda call: True

            def surface_close():
                self.events.append('surface-close')
                close()

            self.surface.receive = receive_control
            self.surface.close = surface_close
            return self.surface

        def link(sock, sdk):
            self.link = real_link(sock, sdk, clock=self.clock, select_fn=self.schedule)
            receive = self.link.receive
            flush = self.link.flush
            close = self.link.close

            def receive_control():
                assert not self.terminal_received, 'extra receive after terminal response'
                self.receives += 1
                return receive()

            def flush_control():
                if self.terminal_received:
                    self.pending_at_terminal = bool(self.link.pending)
                    assert self.pending_at_terminal, 'must force pending final output'
                flush()
                if self.terminal_received:
                    self.collect()
                    self.events.append('terminal-flushed')

            def close_control():
                self.events.append('link-close')
                close()

            self.link.receive = receive_control
            self.link.flush = flush_control
            self.link.close = close_control
            return self.link

        self.shim.connect = lambda path: None
        self.shim.getsockopt = lambda *args: struct.pack('3i', self.child.pid, os.getuid(), 0)
        real_send = self.shim.send

        def send(data):
            assert self.local.fileno() >= 0, 'writer after socket close'
            if mode == 'send-error' and self.surface.world.done:
                raise self.original_error
            return real_send(data)

        self.shim.send = send

        def socket_close():
            if self.local.fileno() >= 0:
                self.collect()
                self.events.append('socket-close')
            self.local.close()

        self.shim.close = socket_close
        socket_proxy = SimpleNamespace(**{name: getattr(socket, name) for name in
            ('AF_UNIX', 'SOCK_STREAM', 'SOL_SOCKET', 'SO_PEERCRED')})
        socket_proxy.socket = lambda *args: self.shim

        def proof(*args, **kwargs):
            self.proof_calls += 1
            self.events.append('proof-refused')
            assert self.child.poll() is not None
            raise BeforeProof('control-path coverage stops before native proof')

        monkeypatch.setattr(ev, 'socket', socket_proxy)
        monkeypatch.setattr(ev, 'subprocess', SimpleNamespace(Popen=popen, run=proof))
        monkeypatch.setattr(ev, 'time', SimpleNamespace(monotonic=self.clock))
        monkeypatch.setattr(ev, 'WorldSurface', surface)
        monkeypatch.setattr(ev, '_EvaluatorLink', link)
        # Short Unix path requirement remains real even though connect is controlled.
        self.short = tempfile.TemporaryDirectory(prefix='et-', dir=os.environ.get('TMPDIR'))
        monkeypatch.setenv('TMPDIR', self.short.name)

    def send_message(self, message):
        raw = encode(ev.WIRE, message)
        data = frame(raw)
        assert self.peer.send(data) == len(data)
        return raw

    def collect(self):
        while True:
            try:
                data = self.peer.recv(65536)
            except BlockingIOError:
                break
            if not data:
                break
            for raw in self.parser.feed(data):
                message = decode(ev.WIRE, raw)
                self.messages.append(message)
                if message['type'] == 'hello':
                    self.send_message(dict(type='hello_reply', body=dict(
                        selection=dict(version='0.1'),
                        correlation_id=message['body']['correlation_id'])))
                elif message['type'] == 'grants' and self.action is None:
                    self.action = self.send_message(dict(type='act', body=dict(
                        grant_epoch=1, action=dict(id='2'*32, coglet=ev.C,
                        capability='sim.cartpole_force', args=dict(direction='right'),
                        deadline=1_000_000, correlation_id='2'*32))))
                elif (message['type'] == 'health' and
                      message['body']['correlation_id'] == '9'*32 and
                      self.mode != 'absent-terminal'):
                    self.send_message(message)

    def schedule(self, reads, writes, errors, timeout):
        self.calls += 1
        assert self.calls <= 12000, 'finite selector watchdog exhausted'
        assert 0 <= timeout <= .010001
        if self.terminal_received:
            assert not reads, 'final flush must never receive'
        self.clock.us += round(timeout * 1_000_000)
        self.collect()
        if self.mode == 'absent-terminal' and self.clock.us - self.last_health >= 500_000:
            self.last_health = self.clock.us
            self.send_message(dict(type='health', body=dict(state='ready',
                clock=self.clock.us, pending=0, correlation_id='3'*32)))
        return select.select(reads, writes, errors, 0)

    def run(self, tmp_path):
        tools = ev.NativeTools(Path(sys.executable).resolve(), Path(sys.executable).resolve())
        return ev.evaluate(tmp_path, tools, b'control-only-not-native-wasm',
                           world='cartpole', seed=42, max_steps=1)

    def assert_cleanup(self, tmp_path, phase, exception):
        diagnostic = json.loads((tmp_path / 'native-failure.json').read_text())
        assert diagnostic['phase'] == phase
        assert diagnostic['exception'] == exception
        assert diagnostic['steps'] == diagnostic['actions'] == 1
        assert diagnostic['barrier_sent']
        assert diagnostic['cleanup']['returncode'] is not None
        assert self.child.poll() is not None
        assert self.local.fileno() == -1
        assert self.link.closed and not self.link.pending
        assert self.surface.state == 'closed'
        assert self.events.index('link-close') < self.events.index('surface-close')
        assert self.events.index('socket-close') < self.events.index('surface-close')
        assert not (tmp_path / 'proof.json').exists()
        return diagnostic

    def close(self):
        # Emergency fixture cleanup is bounded even if a test assertion fails.
        if self.child is not None:
            if self.child.poll() is None:
                self.child.kill()
            self.child.wait(timeout=5)
            self.child.stdin.close()
        self.local.close()
        self.peer.close()
        self.short.cleanup()


@pytest.fixture
def terminal_harness(tmp_path, monkeypatch):
    harnesses = []
    def make(mode):
        harness = TerminalHarness(tmp_path, monkeypatch, mode)
        harnesses.append(harness)
        return harness
    yield make
    for harness in harnesses:
        harness.close()


def barriers(harness):
    return [i for i, message in enumerate(harness.messages)
            if message['type'] == 'health' and
            message['body']['correlation_id'] == '9'*32]


def test_evaluator_terminal_response_flush_and_cleanup(tmp_path, terminal_harness):
    h = terminal_harness('terminal')
    with pytest.raises(BeforeProof, match='stops before native proof'):
        h.run(tmp_path)
    h.assert_cleanup(tmp_path, 'proof', 'BeforeProof')
    assert h.proof_calls == 1
    assert h.receives == 3  # hello_reply, terminal action, matching barrier response
    assert h.pending_at_terminal and h.queued_terminal_outcome
    assert h.surface.world.steps == 1  # durable retry did not repeat the effect
    assert h.calls < 100
    assert len(barriers(h)) == 1
    barrier, = barriers(h)
    outcomes = [i for i, m in enumerate(h.messages)
                if m['type'] == 'event' and m['body']['kind'] == 'action.outcome']
    observations = [i for i, m in enumerate(h.messages)
                    if m['type'] == 'event' and m['body']['kind'] == 'sim.cartpole_state'
                    and m['body']['payload']['done']]
    assert len(outcomes) == 2 and outcomes[0] < barrier < outcomes[1]
    assert len(observations) == 1 and observations[0] < barrier
    assert h.events.index('terminal-received') < h.events.index('terminal-flushed')
    assert h.events.index('terminal-flushed') < h.events.index('socket-close')
    h.parser.eof()


def test_evaluator_blocked_terminal_flush_fails_boundedly(tmp_path, terminal_harness):
    h = terminal_harness('blocked-final')
    with pytest.raises(ProtocolError, match='peer heartbeat timeout'):
        h.run(tmp_path)
    d = h.assert_cleanup(tmp_path, 'loop', 'ProtocolError')
    assert d['cleanup']['supervisor_killed']
    assert h.terminal_received and h.pending_at_terminal
    assert 'terminal-flushed' not in h.events
    assert h.proof_calls == 0 and h.receives == 3
    assert len(barriers(h)) == 1
    assert h.clock.us < 4_000_000 and h.calls < 400


def test_evaluator_done_without_terminal_response_expires_episode(tmp_path, terminal_harness):
    h = terminal_harness('absent-terminal')
    with pytest.raises(TimeoutError, match='episode deadline exceeded'):
        h.run(tmp_path)
    d = h.assert_cleanup(tmp_path, 'loop', 'TimeoutError')
    assert d['cleanup']['supervisor_killed']
    assert h.surface.world.done and not h.terminal_received
    assert h.proof_calls == 0 and len(barriers(h)) == 1
    assert h.link.deadline == pytest.approx(90.01)
    assert h.clock() == pytest.approx(h.link.deadline)
    assert h.calls < 10000


def test_evaluator_failure_cleanup_keeps_original_error(tmp_path, terminal_harness):
    h = terminal_harness('send-error')
    with pytest.raises(BrokenPipeError) as caught:
        h.run(tmp_path)
    assert caught.value is h.original_error
    d = h.assert_cleanup(tmp_path, 'loop', 'BrokenPipeError')
    assert d['errno'] == errno.EPIPE and d['cleanup']['supervisor_killed']
    assert h.proof_calls == 0 and not h.terminal_received
    assert h.receives == 2 and h.calls < 100
    assert len(barriers(h)) == 0  # flag is enqueued, never falsely transmitted
