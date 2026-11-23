import os
import socket
import stat
import tempfile
from pathlib import Path
import pytest


@pytest.fixture
def short_path():
    with tempfile.TemporaryDirectory(prefix='cg-') as directory:
        yield Path(directory)

import pytest

from coglet_surface.codec import encode
from coglet_surface.surface import ProtocolError


def test_framing_handles_fragments_coalescing_and_eof():
    from coglet_surface.transport import Frames, frame
    parser = Frames()
    a, b = b'abc', b'defg'
    wire = frame(a) + frame(b)
    assert parser.feed(wire[:2]) == []
    assert parser.feed(wire[2:6]) == []
    assert parser.feed(wire[6:]) == [a, b]
    parser.eof()
    for n in (0, 1048577):
        with pytest.raises(ProtocolError):
            Frames().feed(n.to_bytes(4, 'big'))
    parser.feed(b'\x00')
    with pytest.raises(ProtocolError):
        parser.eof()


def test_transport_revocation_drops_unsent_private_frames_and_fails_partial():
    from coglet_surface.transport import Output
    from test_surface import CID
    private = encode('surface::surface-message', dict(type='read_reply', body=dict(
        correlation_id=CID, ok=True, value={'secret': 1}, snapshot=1)))
    output = Output()
    output.append(private, 0)
    output.revoke(1)
    assert not output
    output.append(private, 1)
    output.consume(1)
    with pytest.raises(ProtocolError, match='partial'):
        output.revoke(2)


def test_private_socket_permissions_and_peer_credentials(short_path):
    from coglet_surface.transport import UnixListener
    directory = short_path / 'private'
    directory.mkdir(mode=0o700)
    path = directory / 'surface.sock'
    with UnixListener(path, runtime_uid=os.getuid()) as listener:
        assert stat.S_IMODE(path.stat().st_mode) == 0o600
        with socket.socket(socket.AF_UNIX) as client:
            client.connect(str(path))
            with listener.accept() as peer:
                assert peer.family == socket.AF_UNIX
    assert not path.exists()
    directory.chmod(0o755)
    with pytest.raises(PermissionError):
        with UnixListener(path, runtime_uid=os.getuid()):
            pass


def test_wrong_socket_peer_uid_is_not_trusted(tmp_path):
    from coglet_surface.transport import UnixListener
    tmp_path.chmod(0o700)
    with UnixListener(tmp_path / 's', runtime_uid=os.getuid() + 1) as listener:
        with socket.socket(socket.AF_UNIX) as client:
            client.connect(str(tmp_path / 's'))
            with pytest.raises(PermissionError):
                listener.accept()
