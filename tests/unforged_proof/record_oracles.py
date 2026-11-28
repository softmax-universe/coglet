#!/usr/bin/env python3
"""Recapture complete local replay/fork oracles; not hosted qualification.

The existing fixture is a genuine cogletd/WAMR Gridbot recording. Its recorded
adapter outcomes are synthetic world outcomes; no model outcomes are present.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--driver', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    driver = args.driver.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    raw = gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes())
    source = out/'source-trace.json'
    source.write_bytes(raw)
    request = b'{"receipt_index":0,"result":{"what_if":true}}'
    (out/'fork-request.json').write_bytes(request)
    report = {'qualification': 'local Runtime replay/fork oracles only; NOT stage2 PASS',
              'driver_sha256': hashlib.sha256(driver.read_bytes()).hexdigest(),
              'coglet_head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT, text=True).strip(),
              'source_trace_sha256': hashlib.sha256(raw).hexdigest(), 'paths': {}}
    for label, command, data in [('replay', 'record', None), ('fork', 'fork-trace', request)]:
        result = subprocess.run([str(driver), command, str(source)], input=data, capture_output=True, timeout=120)
        if result.returncode:
            raise RuntimeError(result.stderr.decode(errors='replace'))
        (out/f'{label}-trace.json').write_bytes(result.stdout)
        trace = json.loads(result.stdout)
        report['paths'][label] = dict(
            trace_sha256=hashlib.sha256(result.stdout).hexdigest(),
            steps=[dict(step=r['step'], tree=r['output_hash']) for r in trace['records']],
            actions=sum(len(r['actions']) for r in trace['records']),
            llm_requests=sum(len(r['llm_requests']) for r in trace['records']),
            boundary_changes=sum(len(r['boundary']) for r in trace['records']),
            final_hash=trace['final_hash'])
    if report['paths']['replay']['final_hash'] == report['paths']['fork']['final_hash']:
        raise RuntimeError('fork must diverge')
    (out/'result.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
