#!/usr/bin/env python3
"""Actual same-client SYNTHETIC resolver controls, not private or image-upgrade proof.

Use a disposable existing native synthetic model probe only. Adds a fresh input
commit identity (unchanged tree) and one harmless resolver metadata file; never
changes resolver strategy, engine or driver. Retains exact reachable Git objects.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
from hosted_receipts import Refused
from resolver_controls import METADATA, verify_delta, verify_sequence


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ('probe','trace','driver','bridge','output'):
        ap.add_argument('--'+name,type=Path,required=True)
    for name in ('expected-resolver','expected-image','expected-endpoint','expected-driver-sha256','expected-bridge-sha256'):
        ap.add_argument('--'+name,required=True)
    ap.add_argument('--git', type=Path, required=True, help='task-private Git >=2.45 with reftable')
    ap.add_argument('--synthetic-transport-only',action='store_true')
    a=ap.parse_args()
    if not a.synthetic_transport_only:raise Refused('synthetic-only acknowledgement required')
    for path,pin in [(a.driver,a.expected_driver_sha256),(a.bridge,a.expected_bridge_sha256)]:
        if hashlib.sha256(path.read_bytes()).hexdigest()!=pin:raise Refused('independent binary pin differs')
    if not re.fullmatch('[0-9a-f]{64}',a.expected_resolver) or not re.fullmatch('[0-9a-f]{64}',a.expected_endpoint):
        raise Refused('full independent identities required')
    if not re.fullmatch('native@sha256:[0-9a-f]{64}',a.expected_image):raise Refused('native immutable image pin required')
    clean={k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
    local_path=str(a.git.resolve().parent)+os.pathsep+clean.get('PATH','')
    clean['PATH']=local_path
    bridge_env=dict(os.environ,PATH=local_path)
    clean.update(GIT_AUTHOR_NAME='Coglet synthetic proof',GIT_AUTHOR_EMAIL='proof@example.invalid',GIT_COMMITTER_NAME='Coglet synthetic proof',GIT_COMMITTER_EMAIL='proof@example.invalid')
    def command(args,data=None):
        p=subprocess.run([str(x) for x in args],input=data,capture_output=True,env=clean,timeout=120)
        if p.returncode:raise Refused('local command failed (raw diagnostics suppressed)')
        return p.stdout
    # Prove this is the known synthetic model, not private user/device data.
    trace=json.loads(a.trace.read_bytes())
    reference=json.loads(command([a.driver.resolve(),'record-model',Path(__file__).resolve().parents[2]]))
    for key in ('coglet','code_hash','manifest_hash','initial_files','initial_hash'):
        if trace[key]!=reference[key]:raise Refused('not synthetic model oracle')
    verified=json.loads(command([a.driver.resolve(),'record',a.trace.resolve()]))
    # Existing native record() verifies the complete deterministic journal;
    # measured wall-time and its derived chain hashes are regenerated per run.
    # Retain both originals, never rewrite their telemetry or hash chains.
    if [r['output_hash'] for r in verified['records']] != [r['output_hash'] for r in trace['records']]:
        raise Refused('genuine native recorded oracle differs')
    previous=json.loads((a.probe/'result.json').read_bytes())
    if previous['resolver']!=a.expected_resolver:raise Refused('retained resolver pin differs')
    step=previous['step'] if 'step' in previous else 1
    expected=trace['records'][step]['output_hash']
    if previous['recorded_output_tree']!=expected:raise Refused('prior result not bound to oracle')
    out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
    shutil.copytree(a.probe/'os',out/'os')
    gd=out/'os/dev'
    def git(*args,data=None):return command(['git','--no-replace-objects',f'--git-dir={gd}',*args],data)
    resolver_a=a.expected_resolver
    old=git('ls-tree','-z',f'{resolver_a}^{{tree}}')
    nonce=secrets.token_hex(32)
    metadata=b'coglet-stage2-resolver-control-v1\n'+nonce.encode()+b'\n'
    blob=git('hash-object','-w','--stdin',data=metadata).strip()
    tree_b=git('mktree','-z',data=old+b'100644 blob '+blob+b'\t'+METADATA.encode()+b'\0').decode().strip()
    resolver_b=git('commit-tree',tree_b,'-p',resolver_a,data=b'synthetic resolver revision '+nonce.encode()+b'\n').decode().strip()
    verify_delta(old,git('ls-tree','-z',tree_b),git('cat-file','blob',blob.decode()))
    program=previous['program']
    if git('cat-file','blob',f'{program}:proof-driver')!=a.driver.read_bytes():raise Refused('retained program driver differs')
    input_tree=git('rev-parse',f"{previous['input']}^{{tree}}").decode().strip()
    from prefix_inputs import step_payload
    payload=step_payload(trace,step)
    (out/'prefix.json').write_text(json.dumps(payload['prefix']))
    snapshot=json.loads(command([a.driver.resolve(),'snapshot',out/'prefix.json']))
    expected_files={p.removeprefix('/'):bytes.fromhex(value) for p,value in snapshot.items()}
    actual_files={}
    for row in git('ls-tree','-rz',input_tree).split(b'\0'):
        if not row:continue
        descriptor,name=row.split(b'\t',1);mode,kind,oid=descriptor.split(b' ')
        if mode!=b'100644' or kind!=b'blob':raise Refused('nonregular synthetic input')
        actual_files[name.decode()]=git('cat-file','blob',oid.decode())
    actual_payload=actual_files.pop('proof-input.json',b'')
    if json.loads(actual_payload)!=payload or actual_files!=expected_files:
        raise Refused('retained input differs from exact genuine native prefix/current input')
    inputs=git('commit-tree',input_tree,data=b'synthetic resolver control input '+nonce.encode()+b'\n').decode().strip()
    for name,oid in [('program',program),('input',inputs),('resolver-a',resolver_a),('resolver-b',resolver_b)]:
        git('update-ref','refs/stage2-resolver/'+name,oid)
    pins=dict(program=program,inputs=inputs,resolver_a=resolver_a,resolver_b=resolver_b,endpoint=a.expected_endpoint,image=a.expected_image,tree=expected)
    info=dict(pins=pins,trace_sha256=hashlib.sha256(a.trace.read_bytes()).hexdigest(),step=step,
              driver_sha256=a.expected_driver_sha256,bridge_sha256=a.expected_bridge_sha256,
              source=command(['git','-C',Path(__file__).resolve().parents[2],'rev-parse','HEAD']).decode().strip(),
              qualification='synthetic resolver controls only; NOT stage2 PASS')
    (out/'command.json').write_text(json.dumps(info,indent=2)+'\n')
    (out/'trace.json').write_bytes(a.trace.read_bytes())
    (out/'native-rerecorded.json').write_text(json.dumps(verified,indent=2)+'\n')
    args=[str(a.bridge.resolve()),'synthetic-resolver-controls','--synthetic-transport-only',str(out/'os'),program,inputs,resolver_a,resolver_b,a.expected_endpoint]
    info['command']=args
    (out/'command.json').write_text(json.dumps(info,indent=2)+'\n')
    if not os.environ.get('CAOS_IROH_TICKET'):raise Refused('broker ticket unavailable')
    def redacted(raw):
        for name in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
            if os.environ.get(name):raw=raw.replace(os.environ[name].encode(),b'[REDACTED]')
        return re.sub(rb'caos://[^\s]+',b'caos://[REDACTED]',raw)
    try:p=subprocess.run(args,capture_output=True,timeout=600,env=bridge_env)
    except subprocess.TimeoutExpired as e:
        (out/'bridge.jsonl').write_bytes(redacted(e.stdout or b''))
        (out/'bridge-error.txt').write_bytes(redacted(e.stderr or b''))
        raise Refused('bridge timeout, partial evidence retained') from e
    (out/'bridge.jsonl').write_bytes(redacted(p.stdout));(out/'bridge-error.txt').write_bytes(redacted(p.stderr))
    if p.returncode:raise Refused('bridge failed, partial evidence retained')
    checked=verify_sequence(p.stdout.decode(),**pins)
    # Independent immediate readback from trusted-client Git with replacements disabled.
    for row in checked:
        event=next(json.loads(line) for line in p.stdout.splitlines() if json.loads(line).get('label')==row['label'])
        if git('cat-file','commit',row['commit']).decode()!=event['raw_commit'] or git('notes','--ref=receipts','show',row['commit']).decode()!=event['raw_receipt']:
            raise Refused('trusted store readback differs')
    objects=out/'objects';objects.mkdir()
    roots=[program,inputs,resolver_a,resolver_b]+[r['commit'] for r in checked]
    oids=sorted({line.split()[0] for line in git('rev-list','--objects',*roots).decode().splitlines()})
    index={}
    for oid in oids:
        kind=git('cat-file','-t',oid).decode().strip();raw=git('cat-file',kind,oid)
        if hashlib.sha256(kind.encode()+b' '+str(len(raw)).encode()+b'\0'+raw).hexdigest()!=oid:raise Refused('raw object hash differs')
        (objects/oid).write_bytes(raw);index[oid]=kind
    (out/'objects.json').write_text(json.dumps(index,indent=2)+'\n')
    result=dict(observations=checked,qualification=info['qualification'])
    (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))

if __name__=='__main__':main()
