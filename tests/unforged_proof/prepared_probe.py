#!/usr/bin/env python3
"""Exercise two operator grants from held synthetic sources through guarded STANDARD.

Held trace/snapshot acquisition is outside this export seam. This command's keys
are ephemeral operator-testbed keys, not production/device authorization. Native
validation stays local and strips CAOS credentials. NOT stage2 qualification.
"""
import argparse
import gzip
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore, TrustKey, Refused
from guarded_preparation import Sources, prepare
from hosted_export import ExportAuthority
from guarded_transport import RecipeAuthority, scope_for, strict_json, digest
from recorded_closure import require_model_reply_closure
from standard_step import require_fresh
from diagnostic_receipts import verify_isolation_sidecar
from wire_trees import verify_output
from run import ROOT, save, run


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--synthetic-transport-only',action='store_true')
    for name in ('trace','snapshot','driver','bridge','output'):
        ap.add_argument('--'+name,type=Path,required=True)
    for name in ('expected-endpoint','expected-image','expected-bridge-sha256'):
        ap.add_argument('--'+name,required=True)
    ap.add_argument('--coglet',required=True,help='independently expected operator target')
    ap.add_argument('--step',type=int,required=True)
    ap.add_argument('--oracle',choices=('model','gridbot'),required=True)
    ap.add_argument('--deny-export',action='store_true')
    args = ap.parse_args()
    if not args.synthetic_transport_only: raise Refused('synthetic-only prepared export')
    driver,bridge = args.driver.resolve(),args.bridge.resolve()
    source = (ROOT/'tests/unforged_proof/bridge/src/standard.rs').read_text()
    worker = source.split('pub const WORKER: &str = r#"',1)[1].split('"#;',1)[0].encode()
    # Held acquisition is a local read, not a private-payload serialization/upload.
    sources = Sources(trace=args.trace.read_bytes(),snapshot=args.snapshot.read_bytes(),
        driver=driver.read_bytes(),worker=worker,step=args.step,
        sender=digest(b'coglet synthetic operator testbed'),recipient=args.expected_endpoint,
        coglet=args.coglet,image=args.expected_image,salt=secrets.token_hex(32))
    source_scope = sources.scope()
    clean = {k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
    with tempfile.TemporaryDirectory(prefix='prepared-operator-') as authority_dir:
        revoked = set()
        store = ArtifactStore(Path(authority_dir)/'artifacts')
        source_authority = ExportAuthority(store,TrustKey('synthetic-source-export',secrets.token_bytes(32)),revoked)
        recipe_authority = RecipeAuthority(store,TrustKey('synthetic-wire-export',secrets.token_bytes(32)),revoked)
        source_grant = source_authority.issue(source_scope)
        if args.deny_export: revoked.add(source_grant)
        try: source_authority.verify(source_grant,source_scope)
        except Refused:
            if not args.deny_export: raise
            print(json.dumps(dict(denied=True,native_started=False,bridge_started=False,private_staging_writes=0)))
            return
        trace = strict_json(sources.trace)
        require_model_reply_closure(trace)
        def native(*argv):
            result = subprocess.run([str(driver),*map(str,argv)],capture_output=True,env=clean,timeout=120)
            if result.returncode: raise Refused('local native oracle refused')
            return strict_json(result.stdout)
        fixture = native('record-model',ROOT) if args.oracle == 'model' else strict_json(gzip.decompress(
            (ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
        for key in ('coglet','code_hash','manifest_hash','initial_files','initial_hash'):
            if trace[key] != fixture[key]: raise Refused('not selected public synthetic fixture')
        # Validate the exact held trace, not a candidate-mutable path reread.
        source_authority.verify(source_grant,source_scope)
        validation_trace = Path(authority_dir)/'held-trace.json'
        validation_trace.write_bytes(sources.trace)
        verified = native('record',validation_trace)
        if [r['output_hash'] for r in verified['records']] != [r['output_hash'] for r in trace['records']]:
            raise Refused('native recorded trees differ')
        prepared = prepare(sources,source_authority,source_grant,source_scope)
        recipe_scope = scope_for(prepared.recipe,sender=sources.sender,recipient=sources.recipient,coglet=sources.coglet)
        recipe_grant = recipe_authority.issue(recipe_scope)
        result = prepared.execute(parent=authority_dir,name='private-stage',recipe_authority=recipe_authority,
            recipe_grant=recipe_grant,recipe_scope=recipe_scope,driver=driver,bridge=bridge,
            bridge_sha256=args.expected_bridge_sha256,synthetic_transport_only=True)
    out = args.output.resolve()
    out.mkdir(parents=True,exist_ok=False)
    def redact(raw):
        for name in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
            if os.environ.get(name): raw = raw.replace(os.environ[name].encode(),b'[REDACTED]')
        return raw
    (out/'bridge.jsonl').write_bytes(redact(result.pop('stdout')))
    (out/'bridge-error.txt').write_bytes(redact(result.pop('stderr')))
    save(out/'exit.json',result)
    save(out/'source-scope.json',source_scope)
    save(out/'scope.json',recipe_scope)
    save(out/'command.json',dict(argv=sys.argv,source_head=run('git','rev-parse','HEAD',cwd=ROOT).decode().strip(),
        driver_sha256=digest(sources.driver),bridge_sha256=args.expected_bridge_sha256,
        trace_sha256=digest(sources.trace),snapshot_sha256=digest(sources.snapshot),
        qualification='two supplied testbed grant seams; NOT stage2 PASS'))
    if result['refusal']: raise Refused('guarded execution refused; raw observations retained')
    rows = [strict_json(raw) for raw in (out/'bridge.jsonl').read_bytes().splitlines()]
    def one(phase):
        found = [r for r in rows if r.get('phase') == phase]
        if len(found) != 1: raise Refused('missing/ambiguous '+phase)
        return found[0]
    submitted,executed,sidecar = one('submitted'),one('executed'),one('sidecar-readback')
    require_fresh(executed,request=submitted['request'],image=sources.image)
    observations = verify_isolation_sidecar(sidecar['raw_payload'].encode(),sidecar['oid'],request=submitted['request'],salt=sources.salt)
    tree = verify_output(rows,executed['result_oid'])
    if tree != trace['records'][sources.step]['output_hash']: raise Refused('whole recorded output differs')
    summary = dict(request=submitted['request'],output_hash=tree,approvals=result['approvals'],
        observations=observations,sidecar_oid=sidecar['oid'],qualification='actual synthetic prepared export; NOT stage2 PASS')
    save(out/'result.json',summary)
    print(json.dumps(summary))

if __name__ == '__main__': main()
