"""Exercise the real framed-CBOR reference-runtime observer, never expected frames."""
import json
import os
from pathlib import Path
import subprocess
import sys

import cbor2

from test_browser import ROOT


def test_started_child_is_reaped_when_forwarder_startup_is_cancelled(tmp_path, monkeypatch):
    import importlib.util
    import signal
    spec = importlib.util.spec_from_file_location('observer_test', Path(__file__).parents[1] / 'observe.py')
    assert spec is not None and spec.loader is not None
    monkeypatch.syspath_prepend(str(Path(__file__).parents[1]))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    spawned = []
    real_popen = module.subprocess.Popen
    def tracked(*args, **kwargs):
        child = real_popen(*args, **kwargs); spawned.append(child); return child
    def cancel(_self):
        raise KeyboardInterrupt
    monkeypatch.setattr(module.subprocess, 'Popen', tracked)
    monkeypatch.setattr(module.threading.Thread, 'start', cancel)
    old_handler = signal.getsignal(signal.SIGTERM)
    import pytest
    with pytest.raises(KeyboardInterrupt):
        module.observe(os.environ['COGLETD'], tmp_path / 'cancelled.jsonl')
    try:
        assert len(spawned) == 1
        assert spawned[0].poll() is not None
        assert signal.getsignal(signal.SIGTERM) == old_handler
    finally:
        for child in spawned:
            if child.poll() is None:
                child.terminate(); child.wait(timeout=5)


def test_observer_tees_real_cogletd_without_changing_wire_reply(tmp_path):
    request = json.loads((ROOT / 'conformance/abi/cases.json').read_text())[0]['steps'][0]['send']
    raw = cbor2.dumps(request, canonical=True)
    export = tmp_path / 'live.jsonl'
    result = subprocess.run([sys.executable, str(Path(__file__).parents[1] / 'observe.py'),
                             '--records', str(export), '--cogletd', os.environ['COGLETD']],
                            input=len(raw).to_bytes(4, 'big') + raw, capture_output=True, timeout=10)
    assert result.returncode == 0, result.stderr.decode()
    size = int.from_bytes(result.stdout[:4], 'big')
    reply = cbor2.loads(result.stdout[4:4 + size])
    record = json.loads(export.read_text())
    assert record == reply['journal']
    assert record['events'][0]['kind'] == 'runtime.debug_step'
    assert export.stat().st_mode & 0o777 == 0o600
    again = subprocess.run([sys.executable, str(Path(__file__).parents[1] / 'observe.py'),
                            '--records', str(export), '--cogletd', os.environ['COGLETD']],
                           input=b'', capture_output=True, timeout=10)
    assert again.returncode != 0
    assert json.loads(export.read_text()) == record
