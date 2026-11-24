"""Drive every conformance/speech case through the duplex-audio surface host."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

from coglet_sim.host import unix_supervisor

ROOT = Path(__file__).resolve().parents[2]
SIM = ROOT / 'sim'
SPEECH = ROOT / 'conformance/speech'
LABELS = [
    'surface-base',
    'audio-barge-in-stop',
    'audio-over-talk-duck',
    'audio-late-partial-and-stale-revision',
    'speech-handoff-heard-truncation',
    'speech-handoff-work-cancelled',
    'speech-handoff-duplicate-call',
    'speech-handoff-mode-hands-free',
    'speech-handoff-mode-push-to-talk',
]


def _registry(path, wire, control):
    lines = [
        '[[implementations]]',
        'name = "duplex-audio"',
        'suites = ["speech"]',
        'peer_roles = ["runtime"]',
        'transport = "unix"',
        'socket = ' + json.dumps(str(wire)),
    ]
    provision = [sys.executable, str(SIM / 'coglet_sim/provision.py')]
    for label in LABELS:
        lines.append(f'[implementations.fixtures.{label}]')
        if label == 'surface-base' or label.startswith('speech-handoff-'):
            lines.append('setup = ' + json.dumps(provision))
            lines.append(f'[implementations.fixtures.{label}.env]')
            lines.append('COGLET_FIXTURE_CONTROL = ' + json.dumps(str(control)))
        else:
            lines.append('setup = []')
    path.write_text('\n'.join(lines) + '\n')


def test_every_speech_conformance_case_through_duplex_audio(tmp_path):
    report = tmp_path / 'speech.xml'
    registry = tmp_path / 'impls.toml'
    with tempfile.TemporaryDirectory(prefix='coglet-duplex-') as directory:
        with unix_supervisor(directory) as (wire, control):
            _registry(registry, wire, control)
            result = subprocess.run(
                [sys.executable, str(ROOT / 'conformance/run'), '--registry', str(registry),
                 '--junit', str(report)],
                cwd=ROOT, env=os.environ.copy(), capture_output=True, text=True, timeout=120,
            )
            assert result.returncode == 0, result.stdout + result.stderr
    cases = ET.parse(report).findall('.//testcase')
    expected = {json.loads(p.read_text())['name'] for p in SPEECH.glob('*.json')}
    executed = {c.attrib['name'] for c in cases if c.find('skipped') is None}
    assert executed == expected
    assert not any(c.find('failure') is not None or c.find('error') is not None for c in cases)
    assert {c.attrib['name'] for c in cases} == expected
