#!/usr/bin/env python3
"""Run the unchanged Go registry claim and audit every JUnit case identity."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import cases_for, quarantined_ids

QUARANTINE = ROOT / 'ci/quarantine.toml'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--junit', required=True, type=Path)
    args = parser.parse_args()
    text = (ROOT / 'conformance/impls.toml').read_text()
    blocks = ['[[implementations]]' + b for b in text.split('[[implementations]]')[1:]]
    matches = [b for b in blocks if tomllib.loads(b)['implementations'][0]['name'] == 'surface-go-reference']
    if len(matches) != 1:
        raise RuntimeError('expected one Go registration')
    block = matches[0]
    impl = tomllib.loads(block)['implementations'][0]
    if impl['suites'] != ['surface'] or impl['peer_roles'] != ['runtime']:
        raise RuntimeError('unexpected Go conformance claim')
    block = block.replace(json.dumps(impl['command']), json.dumps([str(Path(args.binary).resolve())]))
    with tempfile.TemporaryDirectory(prefix='coglet-go-registry-') as tmp:
        registry = Path(tmp) / 'impls.toml'
        registry.write_text(block)
        subprocess.run([sys.executable, str(ROOT / 'conformance/run'), '--registry', str(registry),
                        '--quarantine', str(QUARANTINE), '--junit', str(args.junit.resolve())], cwd=ROOT, check=True)
    report = ET.parse(args.junit).getroot()
    cases = cases_for(ROOT / 'conformance', 'surface')
    expected = {(impl['name'] + '.surface', c['name']): c for c in cases}
    actual = report.findall('testcase')
    keys = [(c.get('classname'), c.get('name')) for c in actual]
    if len(keys) != len(set(keys)) or set(keys) != set(expected):
        raise RuntimeError('missing/duplicate/unexpected JUnit identities')
    quarantined = quarantined_ids(QUARANTINE)
    passed = skipped = held = 0
    for c in actual:
        case = expected[(c.get('classname'), c.get('name'))]
        if c.find('failure') is not None or c.find('error') is not None:
            raise RuntimeError('failing case')
        skip = c.find('skipped')
        if case.get('peer_role', 'runtime') not in impl['peer_roles']:
            if skip is None or 'peer_role unsupported:' not in skip.get('message', ''):
                raise RuntimeError('unsupported role not explicitly skipped')
            skipped += 1
        else:
            ident = f"{c.get('classname')}/{c.get('name')}"
            if skip is not None and ident in quarantined and 'quarantined (ci/quarantine.toml)' in skip.get('message', ''):
                held += 1  # skipped by ci/quarantine.toml; the stability workflow still runs it
            elif skip is not None:
                raise RuntimeError('supported surface case skipped')
            else:
                passed += 1
    if int(report.get('tests')) != len(keys) or int(report.get('failures')) != 0:
        raise RuntimeError('JUnit totals disagree')
    print(f'Go surface identities verified: {passed} passed, {skipped} unsupported-role skips, {held} quarantined')


if __name__ == '__main__':
    main()
