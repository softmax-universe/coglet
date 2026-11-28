#!/usr/bin/env python3
"""Run all steps of synthetic replay/fork archives on existing hosted workers.

Capability evidence only: NO private transport, model, factory, sandbox or final
stage-2 qualification. Each child validates fresh raw receipts and complete trees.
Credential remains environment-only; diagnostics from children are redacted.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]

def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ('unforge','bridge','driver','replay','fork','output'):
        ap.add_argument('--'+name,type=Path,required=True)
    for name in ('expected-endpoint','expected-image','expected-resolver'):
        ap.add_argument('--'+name,required=True)
    args = ap.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True,exist_ok=False)
    head = subprocess.run(['git','-C',str(ROOT),'rev-parse','HEAD'],check=True,capture_output=True,text=True).stdout.strip()
    summary = dict(source_head=head,qualification='synthetic hosted replay/fork capability only; NOT stage2 PASS',steps=[])
    expected = 0
    for label, source in [('replay',args.replay),('fork',args.fork)]:
        source = source.resolve()
        trace = json.loads((source/'trace.json').read_bytes())
        expected += len(trace['records'])
        for index, record in enumerate(trace['records']):
            target = out/f'{label}-{index}'
            cmd = [sys.executable,str(Path(__file__).with_name('hosted_probe.py'))]
            for key in ('unforge','bridge','driver'):
                cmd += ['--'+key,str(getattr(args,key).resolve())]
            for key in ('expected_endpoint','expected_image','expected_resolver'):
                cmd += ['--'+key.replace('_','-'),getattr(args,key)]
            cmd += ['--recording',str(source),'--output',str(target),'--step-index',str(index),
                    '--synthetic-transport-only','--synthetic-gridbot-oracle']
            process = subprocess.run(cmd,capture_output=True,timeout=300)
            (out/f'{label}-{index}-stdout.txt').write_bytes(process.stdout)
            (out/f'{label}-{index}-stderr.txt').write_bytes(process.stderr)
            if process.returncode:
                raise RuntimeError(f'{label} step {index} failed: see redacted child diagnostics')
            result = json.loads((target/'result.json').read_bytes())
            if result['recorded_output_tree'] != record['output_hash']:
                raise RuntimeError('batch expected tree mismatch')
            row = dict(path=label,step=index,expected_tree=record['output_hash'],receipt=result['receipt'],
                       input=result['input'],input_tree=result['input_tree'],program=result['program'])
            summary['steps'].append(row)
            (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
            print(json.dumps(row),flush=True)
    if len(summary['steps']) != expected:
        raise RuntimeError('missing hosted results')
    summary['verified_count'] = expected
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(f'Verified {expected} hosted synthetic step results; NOT stage2 PASS')

if __name__ == '__main__':
    main()
