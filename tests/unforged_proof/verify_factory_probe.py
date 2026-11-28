#!/usr/bin/env python3
"""Independent offline retained factory evidence checker, not server attestation."""
import argparse
import copy
import json
from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'crates/coglet-gate/python'))
from coglet_gate import canonical, digest
from coglet_gate.hosted import step_salt, validate_steps
from guarded_transport import strict_json
from diagnostic_receipts import verify_isolation_sidecar
from standard_step import require_fresh
from wire_trees import verify_output


def verify(rows,step,request,attempt,image,expected):
    def one(phase):
        found=[r for r in rows if r.get('phase')==phase]
        if len(found)!=1:raise ValueError('missing/ambiguous '+phase)
        return found[0]
    submitted,executed,sidecar=one('submitted'),one('executed'),one('sidecar-readback')
    salt=step_salt(request,step['index'],attempt)
    if submitted['salt']!=salt or step['salt']!=salt or submitted['request']!=step['request_oid']:
        raise ValueError('challenge/attempt salt/request mismatch')
    require_fresh(executed,request=submitted['request'],image=image)
    observed=verify_isolation_sidecar(sidecar['raw_payload'].encode(),sidecar['oid'],request=submitted['request'],salt=salt)
    status=[r for r in rows if r.get('phase')=='readback' and r.get('path')==f"/status/{submitted['request']}?all=1"]
    if len(status)!=1 or status[0]['http_status']!=200:raise ValueError('missing completed status')
    body=strict_json(status[0]['body'])
    if body.get('arg_tree')!=submitted['request'] or body.get('out_trace')!=[sidecar['oid']]:
        raise ValueError('sidecar status mismatch')
    actual=verify_output(rows,executed['result_oid'])
    if actual!=expected or actual!=step['tree']:raise ValueError('full ABI tree differs from native oracle')
    return dict(index=step['index'],request=step['request_oid'],tree=actual,sidecar=sidecar['oid'],observed=observed)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--batch',type=Path,required=True);p.add_argument('--expected-image',required=True)
    p.add_argument('--require-artifact',action='store_true')
    args=p.parse_args();root=args.batch
    request=strict_json((root/'request.json').read_bytes())
    evaluation=strict_json((root/'evaluation.json').read_bytes())
    result=strict_json((root/'result.json').read_bytes())
    hosted=evaluation['report']['hosted']
    oracles=list((root/'steps').glob('*/native-oracle.json'))
    if len(oracles)!=1:raise ValueError('one exact retained native oracle required')
    archive=strict_json(oracles[0].read_bytes())
    validate_steps(hosted['steps'],archive,request,hosted['identity'],hosted['attempt'])
    if hosted['identity']['worker_image']!=args.expected_image:raise ValueError('operator image changed')
    if evaluation['request']!=request or evaluation['tools']!=request['tools']:raise ValueError('service request mismatch')
    artifact=root/'artifact.json'
    if artifact.exists():
        raw=artifact.read_bytes(); envelope=strict_json(raw)
        if digest(raw)!=result['artifact'] or envelope['payload']!=evaluation or envelope['kind']!='evaluation':
            raise ValueError('exact content-addressed service artifact differs')
    elif args.require_artifact:raise ValueError('raw service artifact absent')
    checked=[];negatives=0
    for step,record in zip(hosted['steps'],archive['records'],strict=True):
        child=root/'steps'/step['salt'];raw=(child/'bridge.jsonl').read_bytes()
        if digest(raw)!=step['transcript_sha256']:raise ValueError('retained transcript differs')
        rows=[strict_json(line) for line in raw.splitlines()]
        checked.append(verify(rows,step,request,hosted['attempt'],args.expected_image,record['output_hash']))
        # Mutations of real retained receipt/graph bytes are LOCAL refusal tests,
        # never relabeled as remote executions or worker upgrades.
        mutations=[r for r in rows if r.get('phase')!='executed']
        duplicate=rows+[next(r for r in rows if r.get('phase')=='executed')]
        for altered,image in [(mutations,args.expected_image),(duplicate,args.expected_image),(rows,'native@sha256:'+'0'*64)]:
            try:verify(altered,step,request,hosted['attempt'],image,record['output_hash'])
            except ValueError:negatives+=1
            else:raise ValueError('negative evidence was accepted')
    if result['steps']!=len(checked) or not checked or result['qualification']!='fixture-only' or result['second_accept_refused'] is not True or result['production_refused'] is not True:
        raise ValueError('fixture admission result mismatch')
    summary=dict(count=len(checked),results=checked,local_receipt_negatives=negatives,
        raw_artifact_present=artifact.exists(),qualification='retained authenticated-client observations; NOT complete stage2')
    (root/'independent-verification.json').write_bytes(canonical(summary));print(json.dumps(summary))
if __name__=='__main__':main()
