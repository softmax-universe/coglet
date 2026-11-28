#!/usr/bin/env python3
"""Genuine recorded step in existing strict STANDARD sandbox. SYNTHETIC ONLY.

No private-export or factory qualification. No remote resolver is consumed by this
route. Output is the whole ABI filesystem, diagnostics remain separate.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
from prefix_inputs import step_payload
from recorded_closure import require_model_reply_closure
from run import ROOT, run, save
from standard_step import state_files, snapshot_tree
from diagnostic_receipts import verify_isolation_sidecar
from wire_trees import object_id, file_tree, verify_output


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--synthetic-transport-only', action='store_true')
    ap.add_argument('--oracle', choices=('model','gridbot'), required=True)
    for name in ('driver','bridge','trace','output'):
        ap.add_argument('--'+name, type=Path, required=True)
    ap.add_argument('--step', type=int, required=True)
    ap.add_argument('--expected-endpoint', required=True)
    ap.add_argument('--expected-image', required=True)
    args = ap.parse_args()
    if not args.synthetic_transport_only:
        raise SystemExit('refused: complete private transport grant not integrated')
    driver, bridge, trace_path = args.driver.resolve(), args.bridge.resolve(), args.trace.resolve()
    # Credentials are not forwarded into the local recorded-only driver.
    clean = {k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
    def native(*argv):
        p = subprocess.run([str(driver), *map(str,argv)], capture_output=True, env=clean, timeout=120)
        if p.returncode:
            raise RuntimeError('local native oracle refused')
        return json.loads(p.stdout)
    trace = json.loads(trace_path.read_bytes())
    fixture = native('record-model', ROOT) if args.oracle == 'model' else json.loads(gzip.decompress(
        (ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    for key in ('coglet','code_hash','manifest_hash','initial_files','initial_hash'):
        if trace[key] != fixture[key]:
            raise RuntimeError('not the selected synthetic fixture: '+key)
    require_model_reply_closure(trace)
    verified = native('record', trace_path)
    if [r['output_hash'] for r in verified['records']] != [r['output_hash'] for r in trace['records']]:
        raise RuntimeError('native oracle differs')
    payload = step_payload(trace, args.step)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    save(out/'prefix.json', payload['prefix'])
    snapshot = native('snapshot', out/'prefix.json')
    files = state_files(snapshot, payload)
    if snapshot_tree({p[1:]:bytes.fromhex(v) for p,v in snapshot.items()}) != payload['prefix']['final_hash']:
        raise RuntimeError('prior tree differs')
    state = out/'state'
    state.mkdir()
    for path, raw in files.items():
        target = state/path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw)
    salt = secrets.token_hex(32)
    command = [str(bridge), 'synthetic-step','--synthetic-transport-only',args.expected_endpoint,
               args.expected_image,salt,str(driver),str(state)]
    save(out/'command.json', dict(command=command,driver_sha256=hashlib.sha256(driver.read_bytes()).hexdigest(),
        bridge_sha256=hashlib.sha256(bridge.read_bytes()).hexdigest(), trace_sha256=hashlib.sha256(trace_path.read_bytes()).hexdigest(),
        source_head=run('git','rev-parse','HEAD',cwd=ROOT).decode().strip(), step=args.step,
        expected_tree=trace['records'][args.step]['output_hash'], qualification='synthetic strict STANDARD capability only; NOT stage2 PASS'))
    p = subprocess.run(command, capture_output=True, text=True, timeout=240)
    def redact(text):
        for key in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL'):
            if os.environ.get(key): text = text.replace(os.environ[key], '[REDACTED]')
        return re.sub(r'caos://[^\s]+','caos://[REDACTED]',text)
    (out/'bridge.jsonl').write_text(redact(p.stdout))
    (out/'bridge-error.txt').write_text(redact(p.stderr))
    save(out/'exit.json',dict(returncode=p.returncode))
    if p.returncode:
        raise SystemExit('hosted strict step refused; raw observations retained')
    rows = [json.loads(line) for line in p.stdout.splitlines()]
    def one(phase):
        found = [r for r in rows if r.get('phase') == phase]
        if len(found) != 1: raise RuntimeError('missing/ambiguous '+phase)
        return found[0]
    submitted, executed, sidecar = one('submitted'),one('executed'),one('sidecar-readback')
    req = submitted['request']
    if submitted['state'] != file_tree(files) or submitted['driver'] != object_id('blob',driver.read_bytes()):
        raise RuntimeError('submitted driver/state differs from retained bytes')
    source = (ROOT/'tests/unforged_proof/bridge/src/standard.rs').read_text()
    worker = source.split('pub const WORKER: &str = r#"',1)[1].split('"#;',1)[0].encode()
    if submitted['worker'] != object_id('blob',worker):
        raise RuntimeError('submitted worker differs from retained source')
    if executed['request'] != req or executed['cache_hit'] is not False or executed['worker_image'] != args.expected_image:
        raise RuntimeError('execution identity differs')
    observations = verify_isolation_sidecar(sidecar['raw_payload'].encode(), sidecar['oid'], request=req, salt=salt)
    actual = verify_output(rows,executed['result_oid'])
    expected = trace['records'][args.step]['output_hash']
    if actual != expected: raise RuntimeError('whole ABI output differs: '+actual+' != '+expected)
    save(out/'result.json',dict(request=req, result_oid=executed['result_oid'], output_hash=actual,expected_hash=expected,
        sidecar_oid=sidecar['oid'], preexec_observations=observations, qualification='actual recorded step using strict namespace flags; preexec observations writable by trusted driver, NOT adversarial isolation/no-effects or stage2 PASS'))
    print(json.dumps(dict(request=req, step=args.step, output_hash=actual, qualification='NOT stage2 PASS')))

if __name__ == '__main__': main()
