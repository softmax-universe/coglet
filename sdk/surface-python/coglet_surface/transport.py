"""Private local stream transport. No TCP or claimed-identity authentication."""
from collections import deque
import os
from pathlib import Path
import select
import socket
import stat
import struct
import sys
import time

from .surface import MAX_FRAME, ProtocolError, private_frame, authority_frame
from .codec import decode_profile


def frame(raw):
    if not 0 < len(raw) <= MAX_FRAME:
        raise ProtocolError('invalid frame length')
    return len(raw).to_bytes(4, 'big') + raw


class Frames:
    def __init__(self):
        self.buffer = bytearray()
        self.size = None

    def feed(self, data):
        result = []
        view = memoryview(data)
        while view:
            needed = (4 if self.size is None else self.size) - len(self.buffer)
            take = min(needed, len(view))
            self.buffer.extend(view[:take])
            view = view[take:]
            if len(self.buffer) < (4 if self.size is None else self.size):
                continue
            if self.size is None:
                self.size = int.from_bytes(self.buffer, 'big')
                self.buffer.clear()
                if not 0 < self.size <= MAX_FRAME:
                    raise ProtocolError('invalid frame length')
            else:
                result.append(bytes(self.buffer))
                self.buffer.clear()
                self.size = None
        return result

    def eof(self):
        if self.buffer or self.size is not None:
            raise ProtocolError('EOF mid-frame')


class Output:
    """Bounded output queue retaining revocation metadata through partial writes."""
    def __init__(self):
        self.frames = deque()
        self.offset = 0

    def __bool__(self):
        return bool(self.frames)

    def append(self, raw, generation):
        if len(self.frames) >= 128:
            raise ProtocolError('resource_exhausted: transport queue overflow')
        item = (frame(raw), generation, private_frame(raw), authority_frame(raw))
        message = decode_profile(raw)
        if message['type'] == 'kill':
            # Never interrupt a partially transmitted frame. Kill/health are
            # notifications; native stop has already happened independently.
            index = 1 if self.offset else 0
            self.frames.insert(index, item)
        elif message['type'] == 'health' and message['body']['state'] == 'stopped':
            index = 1 if self.offset else 0
            if len(self.frames) > index and decode_profile(self.frames[index][0][4:])['type'] == 'kill':
                index += 1
            self.frames.insert(index, item)
        else:
            self.frames.append(item)

    def revoke(self, generation, *, killed=False):
        def revoked(item):
            return (item[2] and item[1] != generation) or (killed and item[3])
        if self.frames and self.offset and revoked(self.frames[0]):
            raise ProtocolError('revoked partial private frame')
        self.frames = deque(item for item in self.frames if not revoked(item))

    def peek(self):
        return memoryview(self.frames[0][0])[self.offset:]

    def consume(self, count):
        self.offset += count
        if self.offset == len(self.frames[0][0]):
            self.frames.popleft()
            self.offset = 0


class UnixListener:
    """Linux SO_PEERCRED / BSD getpeereid plus optional supervisor PID binding.

    Directory must already exist, owned by the surface user, mode exactly 0700.
    Never unlinks a pre-existing path, and rejects symlink directories.
    """
    def __init__(self, path, *, runtime_uid, runtime_pid=None):
        self.path = Path(path)
        self.uid, self.pid = runtime_uid, runtime_pid
        self.socket = None

    def __enter__(self):
        directory = self.path.parent
        info = directory.lstat()
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                or stat.S_IMODE(info.st_mode) != 0o700 or directory.resolve() != directory.absolute()):
            raise PermissionError('surface socket directory must be private and not symlinked')
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.bind(str(self.path))
            self.socket = sock
            os.chmod(self.path, 0o600)
            self.inode = self.path.lstat().st_ino
            sock.listen(1)
        except BaseException:
            sock.close()
            raise
        return self

    def accept(self):
        peer, _ = self.socket.accept()
        try:
            if sys.platform.startswith('linux'):
                pid, uid, _ = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            elif hasattr(peer, 'getpeereid'):
                uid, _ = peer.getpeereid()
                pid = None
            else:
                raise PermissionError('OS peer credentials unavailable')
            if uid != self.uid or (self.pid is not None and pid != self.pid):
                raise PermissionError('unauthenticated runtime peer')
            return peer
        except BaseException:
            peer.close()
            raise

    def __exit__(self, *_):
        if self.socket is not None:
            self.socket.close()
            if self.path.exists() and self.path.lstat().st_ino == self.inode:
                self.path.unlink()


def serve(surface, read_fd, write_fd, *, hello_id=None, configured=None, poll=None):
    """Run one already-authenticated connection (or test-only stdio adapter).

    The supervisor calls configured after hello_reply to publish effective grants.
    poll runs on the control thread for bounded simulator updates. Native I/O and
    the physical off-switch must not depend on this loop making progress.
    """
    old = {fd: os.get_blocking(fd) for fd in {read_fd, write_fd}}
    pending = Output()
    pending.append(surface.open(hello_id), surface.revocation)
    parser = Frames()
    was_ready = False
    blocked_since = None
    try:
        for fd in old:
            os.set_blocking(fd, False)
        while True:
            if poll:
                poll()
            surface.tick()
            pending.revoke(surface.revocation, killed=bool(surface._get('killed')))
            for raw in surface.drain():
                pending.append(raw, surface.revocation)
            if pending:
                if blocked_since is None:
                    blocked_since = time.monotonic()
                if time.monotonic() - blocked_since >= 3:
                    raise ProtocolError('blocked peer output')
            else:
                blocked_since = None
            readable, writable, _ = select.select([read_fd], [write_fd] if pending else [], [], .01)
            if writable:
                try:
                    sent = os.write(write_fd, pending.peek())
                except BlockingIOError:
                    sent = 0
                if sent:
                    pending.consume(sent)
                    blocked_since = None
            if readable:
                try:
                    data = os.read(read_fd, 65536)
                except BlockingIOError:
                    continue
                if not data:
                    parser.eof()
                    return
                for raw in parser.feed(data):
                    surface.receive(raw)
                    if surface.state == 'ready' and not was_ready:
                        was_ready = True
                        if configured:
                            configured()
    finally:
        try:
            surface.disconnect()
        finally:
            for fd, blocking in old.items():
                os.set_blocking(fd, blocking)
