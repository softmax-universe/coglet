"""Bounded offline native worker. Source and executable are operator-selected.

Linux prlimit applies limits before exec, without preexec_fn in HTTP threads.
No Cargo, provider, live runtime, or browser-supplied filesystem path is used.
"""
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import tempfile
import threading

MAX_ARCHIVE = 128 * 1024 * 1024
MAX_OUTPUT = 2 * 1024 * 1024


def identity(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def open_regular(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise ValueError('Native source must be a bounded regular file')
        return fd, identity(info)
    except BaseException:
        os.close(fd)
        raise


class NativeReplay:
    timeout = 30

    def __init__(self, archive, helper):
        if not Path('/usr/bin/prlimit').is_file() or not Path('/proc/self/fd').is_dir():
            raise ValueError('Native HTTP replay requires Linux prlimit and procfs')
        self.archive = Path(archive).absolute()
        self.helper = Path(helper).absolute()
        self.source_fd, self.source_id = open_regular(self.archive, MAX_ARCHIVE)
        try:
            self.helper_fd, self.helper_id = open_regular(self.helper, MAX_ARCHIVE)
            if not os.fstat(self.helper_fd).st_mode & 0o111:
                raise ValueError('Native helper must be executable')
        except BaseException:
            os.close(self.source_fd)
            if hasattr(self, 'helper_fd'):
                os.close(self.helper_fd)
            raise
        self.gate = threading.Semaphore(1)
        self.lock = threading.Lock()
        self.closed = False
        self.child = None

    def close(self):
        with self.lock:
            self.closed = True
            if self.child is not None and self.child.poll() is None:
                try:
                    os.killpg(self.child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        # Wait for the active request to reap its worker and drop descriptors.
        with self.gate:
            for name in ('source_fd', 'helper_fd'):
                fd = getattr(self, name)
                if fd is not None:
                    os.close(fd)
                    setattr(self, name, None)

    def replay(self, payload):
        if not isinstance(payload, dict) or (payload and set(payload) != {'receipt_index', 'result'}):
            raise ValueError('Invalid native request')
        if len(json.dumps(payload).encode()) > 65536:
            raise ValueError('Native request limit')
        if not self.gate.acquire(blocking=False):
            raise ValueError('Native replay busy')
        try:
            with tempfile.TemporaryDirectory(prefix='coglet-native-') as directory:
                snapshot = Path(directory) / 'trace.json'
                # The held descriptor and pathname must still identify exactly the
                # startup-selected file. Never follow a replacement or symlink.
                with self.lock:
                    if self.closed:
                        raise ValueError('Native replay closed')
                    if (identity(os.stat(self.archive, follow_symlinks=False)) != self.source_id or
                            identity(os.fstat(self.source_fd)) != self.source_id or
                            identity(os.stat(self.helper, follow_symlinks=False)) != self.helper_id or
                            identity(os.fstat(self.helper_fd)) != self.helper_id):
                        raise ValueError('Native source changed; restart to select again')
                    os.lseek(self.source_fd, 0, os.SEEK_SET)
                    with snapshot.open('xb') as target:
                        remaining = MAX_ARCHIVE + 1
                        while remaining:
                            chunk = os.read(self.source_fd, min(1024 * 1024, remaining))
                            if not chunk:
                                break
                            target.write(chunk)
                            remaining -= len(chunk)
                        if remaining == 0 or identity(os.fstat(self.source_fd)) != self.source_id:
                            raise ValueError('Native source changed during snapshot')
                    snapshot.chmod(0o600)
                with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
                    with self.lock:
                        if self.closed:
                            raise ValueError('Native replay closed')
                        child = subprocess.Popen([
                            '/usr/bin/prlimit', '--cpu=15', '--as=2147483648',
                            f'--fsize={MAX_OUTPUT}', '--nofile=64', '--core=0', '--',
                            f'/proc/self/fd/{self.helper_fd}', 'replay', str(snapshot)],
                            stdin=subprocess.PIPE, stdout=out, stderr=err,
                            pass_fds=(self.helper_fd,), start_new_session=True,
                            env={'PATH': '/usr/bin:/bin', 'LANG': 'C', 'TMPDIR': directory},
                            cwd=directory)
                        self.child = child
                    try:
                        child.communicate(json.dumps(payload).encode(), timeout=self.timeout)
                        if child.returncode != 0:
                            raise ValueError('Native worker refused archive/input or exceeded limits')
                        out.seek(0)
                        raw = out.read(MAX_OUTPUT + 1)
                        if len(raw) > MAX_OUTPUT:
                            raise ValueError('Native output limit')
                        report = json.loads(raw)
                        if report.get('verified_native') is not True:
                            raise ValueError('Native verification missing')
                        return report
                    except subprocess.TimeoutExpired as exc:
                        raise ValueError('Native worker time limit') from exc
                    finally:
                        try:
                            os.killpg(child.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        child.wait()
                        with self.lock:
                            self.child = None
        finally:
            self.gate.release()
