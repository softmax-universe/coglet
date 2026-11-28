#!/usr/bin/env python3
"""Recheck one retained authenticated loopback-control execution, not stage2 PASS."""
import argparse
import hashlib
import json
from pathlib import Path
from diagnostic_receipts import verify_loopback_sidecar
from guarded_transport import strict_json
from standard_step import require_fresh
from wire_trees import verify_output

def verify(job,trace,step,*,image,source,driver_hash,bridge_hash):
    rows=[strict_json(line) for line in (job/'bridge.jsonl').read_bytes().splitlines()]
    def one(phase):
        found=[r for r in rows if r.get('phase')==phase]
        if len(found)!=1:raise ValueError('missing/ambiguous '+phase)
        return found[0]
    submitted,executed,sidecar=one('submitted'),one('executed'),one('sidecar-readback')
    request=submitted['request']
    require_fresh(executed,request=request,image=image)
    command=strict_json((job/'command.json').read_bytes())
    raw=trace.read_bytes();oracle=strict_json(raw)
    if (command['source_head']!=source or command['driver_sha256']!=driver_hash
            or command['bridge_sha256']!=bridge_hash or command['trace_sha256']!=hashlib.sha256(raw).hexdigest()):
        raise ValueError('independently pinned source/tool/oracle differs')
    scope=strict_json((job/'scope.json').read_bytes())
    recipe=scope['recipe']
    if recipe['image']!=image or recipe['salt']!=submitted['salt']:raise ValueError('recipe image/salt differs')
    driver=[n for n in recipe['nodes'] if n['path']=='args/driver']
    if len(driver)!=1 or driver[0]['sha256']!=driver_hash:raise ValueError('recipe driver differs')
    status=[r for r in rows if r.get('phase')=='readback' and r['path']==f'/status/{request}?all=1']
    if len(status)!=1 or status[0]['http_status']!=200:raise ValueError('completed status absent')
    body=strict_json(status[0]['body'])
    if body.get('arg_tree')!=request or body.get('out_trace')!=[sidecar['oid']]:raise ValueError('sidecar status binding differs')
    observations=verify_loopback_sidecar(sidecar['raw_payload'].encode(),sidecar['oid'],request=request,salt=submitted['salt'])
    tree=verify_output(rows,executed['result_oid'])
    if tree!=oracle['records'][step]['output_hash']:raise ValueError('whole native recorded tree differs')
    retained=strict_json((job/'result.json').read_bytes())
    if retained['request']!=request or retained['output_hash']!=tree:raise ValueError('retained result differs')
    return dict(request=request,source=source,output_hash=tree,sidecar=sidecar['oid'],step=step,
        observations=observations,qualification='loopback controls only, not universal no-effects or stage2 PASS')

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ('job','trace'):ap.add_argument('--'+name,type=Path,required=True)
    ap.add_argument('--step',type=int,required=True)
    for name in ('expected-image','expected-source','expected-driver-sha256','expected-bridge-sha256'):
        ap.add_argument('--'+name,required=True)
    a=ap.parse_args()
    result=verify(a.job,a.trace,a.step,image=a.expected_image,source=a.expected_source,
        driver_hash=a.expected_driver_sha256,bridge_hash=a.expected_bridge_sha256)
    (a.job/'independent-verification.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result))
if __name__=='__main__':main()
