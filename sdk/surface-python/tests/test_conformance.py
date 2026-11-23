"""Unchanged language-neutral black-box suite, run by existing SDK pytest CI."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

from reference_host import reference_host

ROOT = Path(__file__).resolve().parents[3]
SDK = ROOT / 'sdk/surface-python'


def test_python_reference_surface_conformance(tmp_path):
    report = tmp_path / 'surface.xml'
    registry = tmp_path / 'impls.toml'
    config = json.loads((ROOT / 'spec/fixtures/surface/adapter-config.json').read_text())
    # Keep AF_UNIX paths below platform limits, even under long pytest basetemps.
    with tempfile.TemporaryDirectory(prefix='coglet-') as directory:
        with reference_host(directory, config) as (wire, control):
            registry.write_text('''[[implementations]]
name = "python-reference"
suites = ["surface"]
peer_roles = ["runtime"]
transport = "unix"
socket = ''' + json.dumps(str(wire)) + '''
[implementations.fixtures.surface-base]
setup = [''' + ','.join(json.dumps(v) for v in [sys.executable, str(SDK / 'tests/provision_reference.py')]) + ''']
[implementations.fixtures.surface-base.env]
COGLET_FIXTURE_CONTROL = ''' + json.dumps(str(control)) + '''
[implementations.fixtures.kill-at-100ms.env]
COGLET_FIXTURE_KILL_MS = "100"
''')
            result = subprocess.run([sys.executable, str(ROOT / 'conformance/run'),
                '--registry', str(registry), '--junit', str(report)], cwd=ROOT,
                env=os.environ.copy(), capture_output=True, text=True, timeout=60)
            assert result.returncode == 0, result.stdout + result.stderr
    cases = ET.parse(report).findall('.//testcase')
    fixtures = [json.loads(p.read_text()) for p in (ROOT / 'conformance/surface').glob('*.json')]
    expected = {c['name'] for c in fixtures if c['peer_role'] == 'runtime'}
    executed = {c.attrib['name'] for c in cases if c.find('skipped') is None}
    assert executed == expected
    assert not any(c.find('failure') is not None or c.find('error') is not None for c in cases)


def test_pinned_binding_catalog_is_current():
    assert json.loads((SDK / 'coglet_surface/bindings.json').read_text()) == json.loads(
        (ROOT / 'spec/capabilities/bindings.json').read_text())
