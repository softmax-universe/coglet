"""Unchanged functional suite with ADR-0021 time; live Unix tests remain separate."""
import asyncio
import json
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
SDK = ROOT / 'sdk/surface-python'
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import cases_for, report, run_case, SUITES


def implementation(command=None):
    return dict(name='python-reference', suites=['surface'], peer_roles=['runtime'],
        transport='stdio', fixture_clock=True, ready_line='COGLET_PYTHON_READY',
        command=command or [sys.executable, str(SDK / 'tests/clock_reference.py')],
        fixtures={'surface-base': {}, 'kill-at-100ms': dict(env={'COGLET_FIXTURE_KILL_MS': '100'})})


async def registry_run(output, command=None):
    impl = implementation(command)
    results = [await run_case(impl, 'surface', case) for case in cases_for(SUITES, 'surface')]
    report(output, results)
    return results


def test_python_reference_surface_conformance(tmp_path):
    path = tmp_path / 'surface.xml'
    results = asyncio.run(registry_run(path))
    assert all(row[3] is None for row in results), results
    cases = ET.parse(path).findall('.//testcase')
    fixtures = [json.loads(p.read_text()) for p in (ROOT / 'conformance/surface').glob('*.json')]
    expected = {c['name'] for c in fixtures if c['peer_role'] == 'runtime'}
    executed = {c.attrib['name'] for c in cases if c.find('skipped') is None}
    assert executed == expected
    assert not any(c.find('failure') is not None or c.find('error') is not None for c in cases)


def test_pinned_binding_catalog_is_current():
    assert json.loads((SDK / 'coglet_surface/bindings.json').read_text()) == json.loads(
        (ROOT / 'spec/capabilities/bindings.json').read_text())
