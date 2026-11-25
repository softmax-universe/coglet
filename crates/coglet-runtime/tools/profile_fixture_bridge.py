#!/usr/bin/env python3
"""Opaque stdio/u32be transport for a fresh, authenticated production cogletd.

No CBOR decoding, clock substitution, callback, or semantic fixture handling.
"""
import os
from pathlib import Path
import selectors
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time

LIMIT = 65536  # maximum queued bytes in EACH direction
STARTUP_SECONDS = 20
READY = "COGLET_CONFORMANCE_READY"


def required_file(name):
    value = os.environ.get(name)
    if not value or not Path(value).is_file():
        raise ValueError(f"{name} must name an existing file")
    return value


def configuration():
    if sys.platform != "linux" or not hasattr(socket, "SO_PEERCRED"):
        raise ValueError("Linux SO_PEERCRED authentication required; unsupported platform")
    daemon = required_file("COGLET_FIXTURE_COGLETD")
    if not Path(daemon).is_absolute() or not os.access(daemon, os.X_OK):
        raise ValueError("COGLET_FIXTURE_COGLETD must be absolute and executable")
    profile = os.environ.get("COGLET_CONFORMANCE_CONTRACT_PROFILE", "surface-0.1")
    if profile not in ("surface-0.1", "surface-0.2"):
        raise ValueError("unsupported exact contract profile")
    # Metadata belongs to the harness; only the explicitly trusted setup and
    # binding are given to cogletd. Never derive runtime effects from metadata.
    required_file("COGLET_FIXTURE_CONFIG")
    setup = required_file("COGLET_FIXTURE_RUNTIME_SETUP")
    binding = required_file("COGLET_FIXTURE_RUNTIME_BINDING")
    catalog = os.environ.get("COGLET_FIXTURE_CATALOG")
    pin = os.environ.get("COGLET_FIXTURE_CATALOG_SHA256")
    if (catalog is None) != (pin is None):
        raise ValueError("catalog and exact SHA256 pin must be supplied together")
    options = ["--contract-profile", profile]
    if catalog is not None:
        assert pin is not None
        required_file("COGLET_FIXTURE_CATALOG")
        if len(pin) != 64 or any(c not in "0123456789abcdef" for c in pin):
            raise ValueError("catalog SHA256 must be exactly 64 lowercase hex digits")
        # The real daemon performs digest and schema admission before listening.
        options += ["--trusted-extension-catalog", catalog, pin]
    tmp = os.environ.get("TMPDIR")
    if not tmp or not Path(tmp).is_dir():
        raise ValueError("TMPDIR must explicitly name an existing private scratch root")
    return daemon, binding, setup, options, tmp


def connect_child(child, path):
    deadline = time.monotonic() + STARTUP_SECONDS
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise RuntimeError(f"cogletd exited before connection ({child.returncode})")
        link = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            link.settimeout(min(.2, max(.001, deadline - time.monotonic())))
            link.connect(str(path))
            pid, uid, _gid = struct.unpack("3i", link.getsockopt(
                socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
            if (pid, uid) != (child.pid, os.geteuid()):
                raise RuntimeError("Unix peer is not the actual launched cogletd PID/UID")
            if child.poll() is not None:
                raise RuntimeError("cogletd exited during authentication")
            link.setblocking(False)
            return link
        except (FileNotFoundError, ConnectionRefusedError, TimeoutError):
            link.close()
            time.sleep(.01)
        except BaseException:
            link.close()
            raise
    raise TimeoutError("cogletd startup exceeded 20 seconds")


def relay(link, child):
    """Copy original bytes, preserving all frame boundaries without inspecting them.

    Nonblocking bounded queues propagate backpressure. EOF in either direction
    terminates the fixture (it is not a half-close/request completion signal).
    """
    source, sink = sys.stdin.fileno(), sys.stdout.fileno()
    old = {fd: os.get_blocking(fd) for fd in (source, sink)}
    to_child, to_stdout = bytearray(), bytearray()
    peer_eof = False
    try:
        for fd in old:
            os.set_blocking(fd, False)
        while True:
            if child.poll() is not None:
                raise RuntimeError(f"cogletd exited ({child.returncode})")
            if peer_eof and not to_stdout:
                return
            # select supports pipes AND redirected regular-file stdio on Linux.
            with selectors.SelectSelector() as poll:
                if not peer_eof and len(to_child) < LIMIT:
                    poll.register(source, selectors.EVENT_READ, "stdin")
                if to_stdout:
                    poll.register(sink, selectors.EVENT_WRITE, "stdout")
                events = (selectors.EVENT_READ if not peer_eof and len(to_stdout) < LIMIT else 0)
                if to_child:
                    events |= selectors.EVENT_WRITE
                if events:
                    poll.register(link, events, "socket")
                for key, ready in poll.select(.2):
                    try:
                        if key.data == "stdin":
                            raw = os.read(source, LIMIT - len(to_child))
                            if not raw:
                                return
                            to_child.extend(raw)
                        elif key.data == "stdout":
                            n = os.write(sink, to_stdout)
                            del to_stdout[:n]
                        else:
                            if ready & selectors.EVENT_WRITE:
                                n = link.send(to_child)
                                del to_child[:n]
                            if ready & selectors.EVENT_READ:
                                raw = link.recv(LIMIT - len(to_stdout))
                                if not raw:
                                    if to_child:
                                        raise RuntimeError("peer closed before queued input was sent")
                                    peer_eof = True
                                else:
                                    to_stdout.extend(raw)
                    except BlockingIOError:
                        pass
    finally:
        for fd, blocking in old.items():
            os.set_blocking(fd, blocking)


def stop_child(child):
    # A new session isolates this launch's complete process group.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=2)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait(timeout=2)


def interrupted(signum, _frame):
    raise InterruptedError(f"received signal {signum}")


def main():
    daemon, binding, setup, options, tmp = configuration()
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, interrupted)
    with tempfile.TemporaryDirectory(prefix="cf-", dir=tmp) as directory:
        os.chmod(directory, 0o700)
        path = Path(directory) / "s"
        if len(os.fsencode(path)) > 107:
            raise ValueError("TMPDIR too long for a private Unix socket; choose a shorter scratch root")
        child = subprocess.Popen(
            [daemon, "--surface-service-socket", str(path), binding,
             str(os.geteuid()), str(Path(directory) / "w"), setup, *options],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=None, start_new_session=True)
        try:
            with connect_child(child, path) as link:
                print(READY, file=sys.stderr, flush=True)
                relay(link, child)
        finally:
            stop_child(child)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f"profile fixture bridge: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
