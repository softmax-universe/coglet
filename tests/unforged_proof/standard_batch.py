#!/usr/bin/env python3
"""Serial synthetic strict STANDARD replay/fork controls. NOT stage2 qualification."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
from run import save


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--synthetic-transport-only',action='store_true')
    for name in ('driver','bridge','oracles','output'): ap.add_argument('--'+name,type=Path,required=True)
    ap.add_argument('--expected-endpoint',required=True)
    ap.add_argument('--expected-image',required=True)
    ap.add_argument('--path',choices=('replay','fork'),help='only this path; default both')
    ap.add_argument('--start-step',type=int,default=0,help='explicit partial-batch start; use a new output dir')
    args = ap.parse_args()
    if not args.synthetic_transport_only: raise SystemExit('private transport unavailable')
    output = args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    results = []
    paths = (args.path,) if args.path else ('replay','fork')
    if args.start_step < 0: raise SystemExit('negative start refused')
    for path in paths:
        trace = args.oracles.resolve()/(path+'-trace.json')
        records = json.loads(trace.read_bytes())['records']
        for index, record in enumerate(records):
            if index < args.start_step: continue
            child = output/f'{path}-{index}'
            command = [sys.executable,str(Path(__file__).with_name('standard_probe.py')),
                       '--synthetic-transport-only','--oracle','gridbot','--step',str(index),
                       '--trace',str(trace),'--driver',str(args.driver.resolve()),'--bridge',str(args.bridge.resolve()),
                       '--output',str(child),'--expected-endpoint',args.expected_endpoint,'--expected-image',args.expected_image]
            subprocess.run(command,check=True,timeout=300)
            result = json.loads((child/'result.json').read_bytes())
            if result['output_hash'] != record['output_hash']: raise RuntimeError('child tree differs')
            results.append(dict(path=path,step=index,**result))
            save(output/'summary.json',dict(results=results,qualification='synthetic strict STANDARD capability; NOT stage2 PASS'))
    expected = sum(max(0,len(json.loads((args.oracles/(p+'-trace.json')).read_bytes())['records'])-args.start_step) for p in paths)
    if not expected or len(results) != expected: raise RuntimeError('missing child results')
    print(json.dumps(dict(count=len(results),qualification='NOT stage2 PASS')))

if __name__ == '__main__': main()
