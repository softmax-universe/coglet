#!/usr/bin/env python3
"""Same frozen router cases/oracles against plain Kernel and MockHub.

Exit 2 means incomplete, even when every implemented case passed. No acceptance
case is removed from the default run and unsupported fixture labels stay SKIP.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SUPPORTED = (
    'enroll-reconnect', 'health-minimal', 'enroll-unsupported-version',
    'enroll-one-use', 'enroll-expired', 'enroll-window-closed',
    'enroll-wrong-node', 'start-before-enroll',
    'event-batch-dedup', 'event-offset-conflict', 'event-source-repeat',
    'event-empty-probe', 'event-source-gap', 'event-retry-a-after-b',
    'event-gap', 'event-runtime-wake', 'event-coglet-forged-event', 'event-foreign-coglet',
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/router-conformance.xml')
    args = parser.parse_args()
    if not args.no_build:
        subprocess.run(['cargo', 'build', '--locked', '-j2', '-p', 'coglet-mock',
                        '--features', 'test-controls', '--example', 'router_conformance_peer'],
                       cwd=ROOT, check=True)
    sys.path.insert(0, str(ROOT / 'conformance'))
    from harness import cases_for
    cases = cases_for(ROOT / 'conformance', 'hub')
    names = {case['name'] for case in cases}
    if len(SUPPORTED) != len(set(SUPPORTED)) or set(SUPPORTED) - names:
        raise SystemExit('supported router fixture list is duplicated or stale')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.unlink(missing_ok=True)
    (ROOT / 'target').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='router-conformance-',
                                     dir=os.environ.get('TMPDIR', ROOT / 'target')) as temporary:
        registry = Path(temporary) / 'registry.toml'
        parts = []
        for backend in ('reference', 'mock'):
            parts.append(f'''[[implementations]]
name = "router-{backend}"
suites = ["hub"]
peer_roles = ["device", "hub"]
transport = "stdio"
command = ["target/debug/examples/router_conformance_peer"]
finish_on_eof = true
''')
            for name in SUPPORTED:
                parts.append(f'''[implementations.fixtures."hub.{name}".env]
COGLET_FIXTURE_IMPLEMENTATION = "{backend}"
''')
            parts.append('[implementations.fixtures."hub.reverse-stream"]\n')
        registry.write_text('\n'.join(parts))
        run = subprocess.run([sys.executable, str(ROOT / 'conformance/run'),
                              '--registry', str(registry), '--junit', str(args.output)], cwd=ROOT)
    report = ET.parse(args.output).getroot()
    entries = report.findall('testcase')
    expected = {(f'router-{backend}.hub', case['name'])
                for backend in ('reference', 'mock') for case in cases}
    actual = [(case.get('classname'), case.get('name')) for case in entries]
    if len(actual) != len(expected) or set(actual) != expected:
        raise SystemExit('router conformance lost/duplicated an acceptance case')
    for case in entries:
        supported = case.get('name') in SUPPORTED
        skipped = case.find('skipped') is not None
        if supported == skipped:
            raise SystemExit(f'fixture support drift: {case.get("classname")}/{case.get("name")}')
    summary = {}
    for backend in ('reference', 'mock'):
        scoped = [case for case in entries if case.get('classname') == f'router-{backend}.hub']
        summary[backend] = dict(total=len(scoped),
                               failed=sum(case.find('failure') is not None or case.find('error') is not None for case in scoped),
                               skipped=sum(case.find('skipped') is not None for case in scoped))
        summary[backend]['passed'] = (summary[backend]['total'] - summary[backend]['failed'] - summary[backend]['skipped'])
    print(json.dumps(summary, sort_keys=True))
    if run.returncode or any(s['failed'] for s in summary.values()):
        return 1
    if any(s['skipped'] for s in summary.values()):
        print('INCOMPLETE: full router conformance is not yet implemented.', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
