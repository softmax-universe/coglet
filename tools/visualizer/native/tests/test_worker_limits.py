"""Trusted malicious workers exercise custody/limits, not replay correctness."""
import importlib.util
import os
from pathlib import Path
import threading
import time

import pytest

ROOT = Path(__file__).resolve().parents[2]


def load():
    spec = importlib.util.spec_from_file_location('native_worker_limits', ROOT / 'native_replay.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def worker(tmp_path, code):
    source = tmp_path / 'trace'
    source.write_text('{}')
    helper = tmp_path / 'helper'
    helper.write_text('#!/usr/bin/python3\n' + code)
    helper.chmod(0o700)
    return load().NativeReplay(source, helper)


def test_close_cancels_and_reaps_worker_and_busy_is_bounded(tmp_path):
    native = worker(tmp_path, 'import time\ntime.sleep(60)\n')
    failures = []
    def run():
        try:
            native.replay({})
        except ValueError as error:
            failures.append(str(error))
    thread = threading.Thread(target=run)
    thread.start()
    deadline = time.monotonic() + 5
    try:
        while native.child is None and time.monotonic() < deadline:
            time.sleep(.01)
        child = native.child
        assert child is not None
        with pytest.raises(ValueError, match='busy'):
            native.replay({})
        native.close()
        thread.join(timeout=5)
        assert not thread.is_alive()
        assert child.poll() is not None
        with pytest.raises(ProcessLookupError):
            os.kill(child.pid, 0)
        assert failures
        with pytest.raises(ValueError, match='closed'):
            native.replay({})
    finally:
        native.close()
        thread.join(timeout=5)


@pytest.mark.parametrize('code', [
    'import os\nwhile True: os.write(1, b"x" * 65536)\n',
    'x = bytearray(3 * 1024**3)\n',
    'while True: pass\n',
])
def test_kernel_output_memory_cpu_fences(tmp_path, code):
    native = worker(tmp_path, code)
    start = time.monotonic()
    try:
        with pytest.raises(ValueError, match='exceeded limits'):
            native.replay({})
        assert time.monotonic() - start < 25
        assert native.child is None
    finally:
        native.close()


def test_native_startup_refuses_symlinks_and_nonexecutable(tmp_path):
    module = load()
    source = tmp_path / 'source'
    source.write_text('{}')
    link = tmp_path / 'link'
    link.symlink_to(source)
    with pytest.raises(OSError):
        module.NativeReplay(link, source)
    with pytest.raises(ValueError, match='executable'):
        module.NativeReplay(source, source)


def test_wall_timeout_kills_and_reaps_and_releases_slot(tmp_path):
    native = worker(tmp_path, 'import time\ntime.sleep(60)\n')
    native.timeout = .15
    try:
        for _ in range(2):
            start = time.monotonic()
            with pytest.raises(ValueError, match='time limit'):
                native.replay({})
            assert time.monotonic() - start < 5
            assert native.child is None
    finally:
        native.close()
