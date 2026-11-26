"""Failure evidence must survive native startup and connection failures."""
import json
import os
from pathlib import Path
import sys

import pytest
from coglet_sim.evaluation import NativeTools, evaluate


def tools(tmp_path, body):
    daemon = tmp_path / 'daemon'
    daemon.write_text(f'#!{sys.executable}\n' + body)
    daemon.chmod(0o700)
    return NativeTools(daemon, Path(sys.executable).resolve())


def run_failure(tmp_path, native):
    with pytest.raises((AssertionError, ConnectionError)) as caught:
        evaluate(tmp_path, native, b'wasm', world='cartpole', seed=42)
    return caught.value


def test_startup_failure_retains_bounded_stderr_and_observed_exit(tmp_path):
    native = tools(tmp_path, "import sys\nsys.stderr.write('x' * 20000 + 'TAIL-MARKER'); sys.exit(3)\n")
    error = run_failure(tmp_path, native)
    assert isinstance(error, AssertionError)
    record = json.loads((tmp_path / 'native-failure.json').read_text())
    assert record['phase'] == 'startup'
    assert record['observed_returncode'] == 3
    assert record['stderr_truncated'] is True
    assert len(record['stderr_tail'].encode()) <= 8192
    assert record['stderr_tail'].endswith('TAIL-MARKER')
    assert record['cleanup']['returncode'] == 3
    assert record['cleanup']['supervisor_killed'] is False
    assert any('native failure' in note for note in error.__notes__)


def test_connection_reset_retains_original_exception_and_cleanup(tmp_path):
    # Leave the client's open frame unread, so close deterministically produces
    # ECONNRESET on Linux; remain alive until evaluate's existing cleanup kills us.
    native = tools(tmp_path, '''import os, socket, sys, time
path = sys.argv[2]
os.makedirs(os.path.dirname(path), exist_ok=True)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path); s.listen(1)
c, _ = s.accept()
while not c.recv(1, socket.MSG_PEEK):
    pass
sys.stderr.write('RESET-MARKER\\n'); sys.stderr.flush()
c.close()
time.sleep(30)
''')
    error = run_failure(tmp_path, native)
    assert isinstance(error, ConnectionResetError)
    assert error.errno == 104
    record = json.loads((tmp_path / 'native-failure.json').read_text())
    assert record['phase'] == 'loop'
    assert record['exception'] == 'ConnectionResetError'
    assert record['errno'] == 104
    assert record['observed_returncode'] is None
    assert 'RESET-MARKER' in record['stderr_tail']
    assert record['cleanup']['supervisor_killed'] is True
    assert record['cleanup']['returncode'] == -9
    assert record['actions'] == 0 and record['steps'] == 0
    assert record['last_sent_at'] is not None


def test_diagnostic_write_failure_never_masks_original(tmp_path, monkeypatch):
    native = tools(tmp_path, "import sys\nsys.exit(7)\n")
    original = Path.write_text
    def fail_record(path, *args, **kwargs):
        if path.name.startswith('native-failure'):
            raise OSError('diagnostic disk refusal')
        return original(path, *args, **kwargs)
    monkeypatch.setattr(Path, 'write_text', fail_record)
    error = run_failure(tmp_path, native)
    assert isinstance(error, AssertionError)
    assert 'daemon exited' in str(error)
    assert any('diagnostic disk refusal' in note for note in error.__notes__)
