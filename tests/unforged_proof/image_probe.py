#!/usr/bin/env python3
"""Rerunnable public synthetic model image control. NOT private/strict isolation.

Runs new genuine record-model oracle with audited native-only driver; no arbitrary
trace input, credentials removed from all local children. One bridge client owns
all four hosted observations. Independent offline verifier is a separate command.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
from prefix_inputs import step_payload
from standard_step import snapshot_tree
from image_controls import verify_sequence,Refused
from wire_trees import verify_output

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ('driver','bridge','unforge','git','output'):ap.add_argument('--'+name,type=Path,required=True)
    for name in ('driver-sha256','bridge-sha256','unforge-sha256','endpoint','image-a','image-b'):ap.add_argument('--'+name,required=True)
    ap.add_argument('--synthetic-transport-only',action='store_true')
    a=ap.parse_args()
    if not a.synthetic_transport_only:raise Refused('synthetic public model only')
    for path,pin in [(a.driver,a.driver_sha256),(a.bridge,a.bridge_sha256),(a.unforge,a.unforge_sha256)]:
        if hashlib.sha256(path.read_bytes()).hexdigest()!=pin:raise Refused('independent binary pin differs')
    clean={k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
    clean['PATH']=str(a.git.resolve().parent)+os.pathsep+clean.get('PATH','')
    clean.update(GIT_AUTHOR_NAME='Synthetic image proof',GIT_AUTHOR_EMAIL='proof@example.invalid',GIT_COMMITTER_NAME='Synthetic image proof',GIT_COMMITTER_EMAIL='proof@example.invalid')
    def run(cmd,data=None):
        p=subprocess.run([str(x) for x in cmd],input=data,env=clean,capture_output=True,timeout=120)
        if p.returncode:raise Refused('local command failed: '+p.stderr.decode(errors='replace')[:500])
        return p.stdout
    out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
    root=Path(__file__).resolve().parents[2]
    trace=json.loads(run([a.driver.resolve(),'record-model',root]))
    (out/'trace.json').write_text(json.dumps(trace))
    rerecorded=json.loads(run([a.driver.resolve(),'record',out/'trace.json']))
    if [r['output_hash'] for r in trace['records']]!=[r['output_hash'] for r in rerecorded['records']]:raise Refused('native replay oracle differs')
    (out/'native-rerecorded.json').write_text(json.dumps(rerecorded))
    step=1;payload=step_payload(trace,step)
    (out/'prefix.json').write_text(json.dumps(payload['prefix']))
    snapshot=json.loads(run([a.driver.resolve(),'snapshot',out/'prefix.json']))
    files={p.removeprefix('/'):bytes.fromhex(raw) for p,raw in snapshot.items()}
    if snapshot_tree(files)!=payload['prefix']['final_hash']:raise Refused('prefix tree differs')
    files['proof-input.json']=json.dumps(payload).encode()
    state=out/'state';state.mkdir()
    for path,raw in files.items():
        target=state/path;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(raw)
    run([a.unforge.resolve(),'init',out/'os'])
    gd=out/'os/dev'
    def git(*cmd,data=None):return run([a.git.resolve(),'--no-replace-objects',f'--git-dir={gd}',*cmd],data).decode().strip()
    def blob(raw):return git('hash-object','-w','--stdin',data=raw)
    driver=blob(a.driver.read_bytes())
    drivers=git('mktree',data=f'100755 blob {driver}\tproof\n'.encode())
    strategy=b'driver = proof\ndeterministic = true\n'
    strategy_oid=blob(strategy)
    resolver_tree=git('mktree',data=f'040000 tree {drivers}\tdrivers\n100644 blob {strategy_oid}\tstrategy\n'.encode())
    resolver=git('commit-tree',resolver_tree,data=b'synthetic image resolver\n')
    git('update-ref','refs/heads/resolver',resolver)
    (out/'resolver-commit.raw').write_bytes(run([a.git.resolve(),'--no-replace-objects',f'--git-dir={gd}','cat-file','commit',resolver]))
    salt=secrets.token_hex(32)
    cmd=[str(a.bridge.resolve()),'synthetic-image-controls','--synthetic-transport-only',a.endpoint,a.image_a,a.image_b,salt,str(out/'os'),resolver,a.driver_sha256,str(state)]
    info=dict(source=run(['git','-C',root,'rev-parse','HEAD']).decode().strip(),upstream=run(['git','-C',root.parent/'unforged','rev-parse','HEAD']).decode().strip(),
        driver=a.driver_sha256,bridge=a.bridge_sha256,unforge=a.unforge_sha256,resolver=resolver,resolver_tree=resolver_tree,salt=salt,image_a=a.image_a,image_b=a.image_b,endpoint=a.endpoint,
        step=step,tree=trace['records'][step]['output_hash'],command=cmd,qualification='synthetic bounded native image control, NOT strict/private/stage2 PASS')
    (out/'command.json').write_text(json.dumps(info,indent=2))
    env=dict(os.environ,PATH=clean['PATH'])
    def redact(raw):
        for name in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
            if os.environ.get(name):raw=raw.replace(os.environ[name].encode(),b'[REDACTED]')
        return raw
    try:p=subprocess.run(cmd,env=env,capture_output=True,timeout=720)
    except subprocess.TimeoutExpired as e:
        (out/'bridge.jsonl').write_bytes(redact(e.stdout or b''));(out/'bridge-error.txt').write_bytes(redact(e.stderr or b''));raise Refused('timeout partial raw evidence retained') from None
    (out/'bridge.jsonl').write_bytes(redact(p.stdout));(out/'bridge-error.txt').write_bytes(redact(p.stderr))
    if p.returncode:raise Refused('bridge failed, raw evidence retained')
    rows=[json.loads(line) for line in p.stdout.splitlines()]
    observations=[]
    for i,row in enumerate(rows):
        if row.get('phase')!='image-observation':continue
        end=next((j for j in range(i+1,len(rows)) if rows[j].get('phase')=='image-start'),len(rows))
        tree=verify_output(rows[i:end],row['root'])
        observations.append({k:row[k] for k in ('label','request','args','bundle','resolver','driver','salt','image','headers')}|dict(tree=tree))
    result=verify_sequence(observations,image_a=a.image_a,image_b=a.image_b,resolver=resolver,driver=a.driver_sha256,salt=salt,tree=info['tree'])
    (out/'result.json').write_text(json.dumps(dict(summary=result,observations=observations),indent=2))
    print(json.dumps(result))
if __name__=='__main__':main()
