#!/usr/bin/env python3
"""Independently verify retained strict STANDARD transcripts, never stage2 PASS."""
import argparse
import json
from pathlib import Path
from diagnostic_receipts import verify_isolation_sidecar
from standard_step import require_fresh
from wire_trees import verify_output
from run import save


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--batch',type=Path,required=True)
    ap.add_argument('--expected-image',required=True)
    args = ap.parse_args()
    results = []
    for path in sorted(args.batch.glob('*/result.json')):
        child = path.parent
        rows = [json.loads(line) for line in (child/'bridge.jsonl').read_text().splitlines()]
        def one(phase):
            values = [r for r in rows if r.get('phase') == phase]
            if len(values) != 1: raise RuntimeError('missing/ambiguous '+phase)
            return values[0]
        submitted,executed,sidecar = one('submitted'),one('executed'),one('sidecar-readback')
        req = submitted['request']
        require_fresh(executed,request=req,image=args.expected_image)
        observation = verify_isolation_sidecar(sidecar['raw_payload'].encode(),sidecar['oid'],request=req,salt=submitted['salt'])
        status = [r for r in rows if r.get('phase') == 'readback' and r['path'] == f'/status/{req}?all=1']
        if len(status) != 1 or status[0]['http_status'] != 200: raise RuntimeError('missing completed status')
        body = json.loads(status[0]['body'])
        if body.get('arg_tree') != req or body.get('out_trace') != [sidecar['oid']]: raise RuntimeError('status sidecar differs')
        actual = verify_output(rows,executed['result_oid'])
        expected = json.loads((child/'command.json').read_bytes())['expected_tree']
        if actual != expected: raise RuntimeError('ABI tree differs')
        result = json.loads(path.read_bytes())
        if result['request'] != req or result['output_hash'] != actual: raise RuntimeError('retained result differs')
        results.append(dict(child=child.name,request=req,output_hash=actual,sidecar=sidecar['oid'],preexec=observation))
    if not results: raise RuntimeError('no complete transcript')
    save(args.batch/'independent-verification.json',dict(results=results,count=len(results),
        qualification='only enumerated completed strict STANDARD invocations verified; not complete stage2'))
    print(json.dumps(dict(count=len(results),children=[r['child'] for r in results],qualification='NOT stage2 PASS')))

if __name__ == '__main__': main()
