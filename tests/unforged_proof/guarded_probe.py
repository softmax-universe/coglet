#!/usr/bin/env python3
"""Exercise operator-owned grants at real STANDARD crossings, synthetic data ONLY.

Consumes an already recorded/prepared state; does not claim that its previous
preparation was granted. The export boundary below independently verifies retained
bytes and metadata before starting the pinned bridge. Still NOT stage2 PASS.
"""
import argparse
from collections import Counter
import gzip
import hashlib
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore,TrustKey,Refused
from guarded_transport import RecipeAuthority,ApprovalSession,recipe_for,scope_for,run_guarded,strict_json,digest
from prefix_inputs import step_payload
from recorded_closure import require_model_reply_closure
from standard_step import snapshot_tree,require_fresh
from diagnostic_receipts import verify_isolation_sidecar
from wire_trees import file_tree,object_id,verify_output
from run import ROOT,save,run


def retained_state(root):
    files,dirs = {},[]
    def walk(path,prefix='',depth=0):
        if depth > 32 or path.is_symlink(): raise Refused('state directory invalid')
        for entry in sorted(path.iterdir()):
            rel = prefix+entry.name
            meta = entry.lstat()
            if stat.S_ISDIR(meta.st_mode):
                dirs.append(rel)
                walk(entry,rel+'/',depth+1)
            elif stat.S_ISREG(meta.st_mode):
                fd = os.open(entry,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
                try:
                    if not stat.S_ISREG(os.fstat(fd).st_mode): raise Refused('state file changed')
                    with os.fdopen(fd,'rb',closefd=False) as stream: raw = stream.read(64*1024*1024+1)
                finally: os.close(fd)
                files[rel] = raw
                if sum(map(len,files.values())) > 64*1024*1024: raise Refused('state budget exceeded')
            else: raise Refused('nonregular retained state')
    walk(root)
    return files,dirs


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--synthetic-transport-only',action='store_true')
    for name in ('driver','bridge','state','trace','output'): ap.add_argument('--'+name,type=Path,required=True)
    for name in ('expected-endpoint','expected-image','expected-bridge-sha256'): ap.add_argument('--'+name,required=True)
    ap.add_argument('--oracle',choices=('model','gridbot'),required=True)
    ap.add_argument('--step',type=int,required=True)
    ap.add_argument('--deny-export',action='store_true')
    ap.add_argument('--revoke-at-sequence',type=int)
    args = ap.parse_args()
    if not args.synthetic_transport_only: raise SystemExit('refused: synthetic-only transport increment')
    driver,bridge = args.driver.resolve(),args.bridge.resolve()
    trace = strict_json(args.trace.read_bytes())
    require_model_reply_closure(trace)
    clean = {k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
    def native(*argv):
        result = subprocess.run([str(driver),*map(str,argv)],capture_output=True,env=clean,timeout=120)
        if result.returncode: raise Refused('native oracle refused')
        return strict_json(result.stdout)
    fixture = native('record-model',ROOT) if args.oracle == 'model' else strict_json(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    for key in ('coglet','code_hash','manifest_hash','initial_files','initial_hash'):
        if trace[key] != fixture[key]: raise Refused('not selected synthetic fixture')
    verified = native('record',args.trace.resolve())
    if [r['output_hash'] for r in verified['records']] != [r['output_hash'] for r in trace['records']]: raise Refused('recorded oracle mismatch')
    files,dirs = retained_state(args.state.resolve())
    payload = step_payload(trace,args.step)
    if strict_json(files.get('proof-input.json',b'')) != payload: raise Refused('retained prefix/current input mismatch')
    if snapshot_tree({p:v for p,v in files.items() if p != 'proof-input.json'}) != payload['prefix']['final_hash']:
        raise Refused('retained prior full tree mismatch')
    source = (ROOT/'tests/unforged_proof/bridge/src/standard.rs').read_text()
    worker = source.split('pub const WORKER: &str = r#"',1)[1].split('"#;',1)[0].encode()
    driver_bytes = driver.read_bytes()
    salt = secrets.token_hex(32)
    recipe = recipe_for(files,dirs,driver=driver_bytes,worker=worker,salt=salt,image=args.expected_image)
    # Ephemeral operator-owned testbed key: never given to the bridge/candidate.
    # This explicitly does NOT authorize any real device/private source export.
    sender = digest(b'coglet synthetic operator testbed')
    scope = scope_for(recipe,sender=sender,recipient=args.expected_endpoint,coglet=trace['coglet'])
    command = [str(bridge),'guarded-step','--synthetic-transport-only',args.expected_endpoint,args.expected_image,salt,str(driver),str(args.state.resolve())]
    with tempfile.TemporaryDirectory(prefix='guarded-authority-') as authority_dir:
        revoked = set()
        authority = RecipeAuthority(ArtifactStore(authority_dir),TrustKey('synthetic-operator-export',secrets.token_bytes(32)),revoked)
        grant = authority.issue(scope)
        if args.deny_export: revoked.add(grant)
        try:
            retained = {'args/state/'+p:raw for p,raw in files.items()}
            retained.update({'args/driver':driver_bytes,'args/worker1':worker,'args/salt':salt.encode(),
                'args/required-native-worker-image':args.expected_image.encode(),'args/required-pool':b'unforged'})
            session = ApprovalSession(authority,grant,scope,retained_blobs=retained)
        except Refused:
            if not args.deny_export: raise
            print(json.dumps(dict(denied=True,bridge_started=False,private_staging_writes=0,qualification='initial denial; no hosted job')))
            return
        def revoke(event):
            if event.get('phase') == 'authorize' and event['sequence'] == args.revoke_at_sequence:
                revoked.add(grant)
        result = run_guarded(command,session,bridge_sha256=args.expected_bridge_sha256,on_event=revoke)
    out = args.output.resolve()
    out.mkdir(parents=True,exist_ok=False)
    def redact(raw):
        for key in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
            if os.environ.get(key): raw = raw.replace(os.environ[key].encode(),b'[REDACTED]')
        return raw
    (out/'bridge.jsonl').write_bytes(redact(result.pop('stdout')))
    (out/'bridge-error.txt').write_bytes(redact(result.pop('stderr')))
    save(out/'exit.json',result)
    save(out/'scope.json',scope)
    save(out/'command.json',dict(command=command,source_head=run('git','rev-parse','HEAD',cwd=ROOT).decode().strip(),driver_sha256=digest(driver_bytes),bridge_sha256=args.expected_bridge_sha256,trace_sha256=digest(args.trace.read_bytes()),step=args.step,expected_tree=trace['records'][args.step]['output_hash'],qualification='synthetic operator grant seam; NOT stage2 PASS'))
    if result['refusal']:
        if args.revoke_at_sequence:
            print(json.dumps(result))
            return
        raise SystemExit('guarded bridge refused; observations retained')
    rows = [strict_json(line) for line in (out/'bridge.jsonl').read_bytes().splitlines()]
    def one(phase):
        found = [r for r in rows if r.get('phase') == phase]
        if len(found) != 1: raise Refused('missing/ambiguous '+phase)
        return found[0]
    submitted,executed,sidecar = one('submitted'),one('executed'),one('sidecar-readback')
    if submitted['state'] != file_tree(files) or submitted['driver'] != object_id('blob',driver_bytes) or submitted['worker'] != object_id('blob',worker):
        raise Refused('submitted bytes differ')
    require_fresh(executed,request=submitted['request'],image=args.expected_image)
    observations = verify_isolation_sidecar(sidecar['raw_payload'].encode(),sidecar['oid'],request=submitted['request'],salt=salt)
    tree = verify_output(rows,executed['result_oid'])
    if tree != trace['records'][args.step]['output_hash']: raise Refused('whole recorded output differs')
    summary = dict(request=submitted['request'],output_hash=tree,approvals=result['approvals'],operations=dict(Counter(r['operation'] for r in rows if r.get('phase')=='authorize')),sidecar_oid=sidecar['oid'],observations=observations,qualification='actual guarded synthetic export and execution; NOT stage2 PASS')
    save(out/'result.json',summary)
    print(json.dumps(summary))

if __name__ == '__main__': main()
