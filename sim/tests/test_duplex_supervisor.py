"""Supervisor recovery uses real disconnected Unix peers, not mocked errors."""
import json
import socket
import tempfile
import threading
import pytest

from coglet_surface.codec import decode
from coglet_surface.surface import WIRE
from coglet_sim import host


def setup(control, case='duplicate-call'):
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(2)
        peer.connect(str(control))
        peer.sendall(json.dumps({'case': case}).encode() + b'\n')
        assert peer.recv(32) == b'ready\n'


def test_disconnect_does_not_abandon_listeners(monkeypatch):
    entered, disconnected = threading.Event(), threading.Event()
    original = host.serve
    calls = []

    def serve(*args, **kwargs):
        calls.append(1)
        if len(calls) == 1:
            entered.set()
            assert disconnected.wait(2)
        return original(*args, **kwargs)

    monkeypatch.setattr(host, 'serve', serve)
    with tempfile.TemporaryDirectory(prefix='duplex-recover-') as directory:
        with host.unix_supervisor(directory) as (wire, control):
            setup(control)
            with socket.socket(socket.AF_UNIX) as peer:
                peer.connect(str(wire))
                assert entered.wait(2)
            # Real EOF before the pending hello write produces BrokenPipeError.
            disconnected.set()
            setup(control)
            with socket.socket(socket.AF_UNIX) as peer:
                peer.settimeout(2)
                peer.connect(str(wire))
                with peer.makefile('rb') as source:
                    size = int.from_bytes(source.read(4), 'big')
                    message = decode(WIRE, source.read(size))
                    assert message['type'] == 'hello'
    assert len(calls) == 2


@pytest.mark.parametrize('channel,partial', [('control', b''), ('control', b'{'),
                                           ('wire', b'\x00\x00\x00\x10\xa0')])
def test_truncated_peer_does_not_poison_next_scenario(channel, partial):
    with tempfile.TemporaryDirectory(prefix='duplex-truncated-') as directory:
        with host.unix_supervisor(directory) as (wire, control):
            if channel == 'wire':
                setup(control)
            with socket.socket(socket.AF_UNIX) as peer:
                peer.settimeout(2)
                peer.connect(str(control if channel == 'control' else wire))
                if channel == 'wire':
                    assert peer.recv(4096)  # drain hello before sending a partial frame
                peer.sendall(partial)
            setup(control)
            with socket.socket(socket.AF_UNIX) as peer:
                peer.settimeout(2)
                peer.connect(str(wire))
                assert peer.recv(4096)
