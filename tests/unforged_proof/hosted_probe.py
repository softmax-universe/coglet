#!/usr/bin/env python3
"""Hosted recorded-counter capability probe, NOT the stage-2 qualification suite.

Build the bridge against the pinned sibling Unforged checkout; provide its path,
static driver, stage-1 recording and independent expected endpoint/image pins.
Only synthetic stage-1 data is permitted. Do not use this command on device data.
The grant enumerates retained program/input/resolver objects, NOT all generated
CAOS transport objects (seed/meta/worker/argument wrappers). Therefore this probe
is deliberately unavailable as a private-export gate; the explicit synthetic
flag acknowledges that incomplete transport-closure integration.
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
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore, TrustKey
from hosted_export import ExportAuthority, export_private
from hosted_receipts import validate_invocation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('unforge', 'bridge', 'driver', 'recording', 'output'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--step-index', type=int, default=0)
    parser.add_argument('--synthetic-gridbot-oracle', action='store_true',
                        help='allow only code/manifest/initial state of the committed synthetic Gridbot fixture')
    parser.add_argument('--expected-endpoint', required=True)
    parser.add_argument('--expected-image', required=True)
    parser.add_argument('--expected-resolver', required=True)
    parser.add_argument('--synthetic-transport-only', action='store_true',
                        help='acknowledge this probe does NOT enforce the full transport export closure')
    args = parser.parse_args()
    if not args.synthetic_transport_only:
        raise SystemExit('refused: full transport-closure authorization not yet implemented; synthetic capability probe only')
    ticket = os.environ.get('CAOS_IROH_TICKET', '').strip()
    if not ticket:
        raise SystemExit('broker ticket missing; probe unavailable')
    env = dict(os.environ)
    env.update(UNFORGED_CAOS_URL=ticket if ticket.startswith('caos://') else 'caos://'+ticket,
               UNFORGED_CAOS_BASE='unforged', GIT_NO_REPLACE_OBJECTS='1')
    clean = {k:v for k,v in env.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
    clean.update(GIT_AUTHOR_NAME='Coglet synthetic proof', GIT_AUTHOR_EMAIL='proof@example.invalid',
                 GIT_COMMITTER_NAME='Coglet synthetic proof', GIT_COMMITTER_EMAIL='proof@example.invalid',
                 GIT_AUTHOR_DATE='2026-11-28T00:00:00Z', GIT_COMMITTER_DATE='2026-11-28T00:00:00Z')
    out = args.output.resolve()
    out.mkdir(exist_ok=False, parents=True)
    recording = args.recording.resolve()
    trace = json.loads((recording/'trace.json').read_bytes())
    if args.synthetic_gridbot_oracle:
        import gzip
        fixture = json.loads(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
        for key in ('coglet','code_hash','manifest_hash','initial_files','initial_hash'):
            if trace[key] != fixture[key]:
                raise RuntimeError('not the committed synthetic Gridbot fixture: '+key)
    elif trace['coglet'] != 'coglet:'+'09'*32:
        raise RuntimeError('only synthetic counter recording permitted')
    if not 0 <= args.step_index < len(trace['records']):
        raise RuntimeError('step index outside recording')
    probe = subprocess.run([str(args.bridge.resolve())], env=env, capture_output=True, timeout=90)
    if probe.returncode:
        raise RuntimeError('authenticated native discovery unavailable; diagnostics suppressed')
    discovery = json.loads(probe.stdout)
    if discovery['endpoint_identity'] != args.expected_endpoint:
        raise RuntimeError('authenticated endpoint differs from operator pin')
    if any(image is not None and image != args.expected_image for image in discovery['native_images']):
        raise RuntimeError('advertised native worker differs from operator image pin')
    (out/'discovery.json').write_text(json.dumps(discovery, indent=2)+'\n')
    state = out/'os'
    shutil.copytree(recording/'unforged-os', state)
    gitdir = state/'dev'

    def git(*command, data=None, raw=False):
        process = subprocess.run(['git', '--no-replace-objects', f'--git-dir={gitdir}', *command],
                                 input=data, capture_output=True, env=clean, timeout=30)
        if process.returncode:
            raise RuntimeError('local object inspection failed: '+process.stderr.decode(errors='replace'))
        return process.stdout if raw else process.stdout.decode().strip()

    driver = args.driver.resolve().read_bytes()
    blob = git('hash-object','-w','--stdin',data=driver)
    program_tree = git('mktree',data=f'100755 blob {blob}\tproof-driver\n'.encode())
    input_tree = git('rev-parse',f'refs/proof/input-{args.step_index}')
    program = git('commit-tree',program_tree,data=b'synthetic recorded-step program\n')
    inputs = git('commit-tree',input_tree,data=b'synthetic recorded-step input, no expected output\n')
    # Validate retained closure locally. This is not export-payload serialization.
    # Pin BEFORE upload: resolver content can override the worker base and can
    # contain private engine blobs. Only the independently selected pin is allowed.
    resolver_pin = git('rev-parse','refs/heads/resolver')
    if resolver_pin != args.expected_resolver:
        raise RuntimeError('resolver differs from operator pin before upload')
    oids = sorted(set(line.split()[0] for line in git('rev-list','--objects',program,inputs,resolver_pin).splitlines()))
    objects = []
    for oid in oids:
        kind = git('cat-file','-t',oid)
        raw = git('cat-file',kind,oid,raw=True)
        if hashlib.sha256(kind.encode()+b' '+str(len(raw)).encode()+b'\0'+raw).hexdigest() != oid:
            raise RuntimeError('export object hash mismatch')
        objects.append(dict(oid=oid,kind=kind,length=len(raw)))
    scope = dict(sender=hashlib.sha256(b'coglet-stage2-synthetic-coordinator').hexdigest(),
                 recipient=args.expected_endpoint, coglet=trace['coglet'],
                 operations=['export','read'],objects=objects)
    # This operator-owned probe issues an explicit synthetic testbed grant.
    # No factory/candidate gets the key; no production/device consent is implied.
    authority = ExportAuthority(ArtifactStore(out/'grants'),
                                TrustKey('synthetic-testbed-export', secrets.token_bytes(32)), set())
    grant = authority.issue(scope)
    (out/'scope.json').write_text(json.dumps(dict(scope=scope,grant=grant),indent=2)+'\n')
    command = [str(args.unforge.resolve()),'--root',str(state),'stream','exec',program,inputs,
               '--no-cache','--bin','proof-driver']

    def upload(_):
        try:
            process = subprocess.run(command, env=env, capture_output=True, text=True, timeout=180)
            text = process.stdout+'\n'+process.stderr
            text = text.replace(env['UNFORGED_CAOS_URL'],'[CAOS_ENDPOINT]').replace(ticket,'[CAOS_TICKET]')
            text = re.sub(r'caos://[^\s]+','caos://[REDACTED]',text)
            result = dict(returncode=process.returncode,redacted_output=text,program=program,input=inputs,
                          input_tree=input_tree,program_tree=program_tree,grant=grant,
                          driver_sha256=hashlib.sha256(driver).hexdigest(),
                          unforge_sha256=hashlib.sha256(args.unforge.read_bytes()).hexdigest(),
                          qualification='one-step capability probe only; NOT stage2 PASS')
        except subprocess.TimeoutExpired:
            result = dict(returncode=124,qualification='probe timeout; no execution evidence')
        (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        if result['returncode']:
            print(json.dumps(result,indent=2))
            return result['returncode']
        match = re.fullmatch(r'CONFLUENCE ([0-9a-f]{64}) tree=([0-9a-f]{64}) memo_hit=false resolver=([0-9a-f]{64})\s*',text)
        if not match:
            raise RuntimeError('missing/ambiguous fresh execution result')
        commit, tree, resolver = match.groups()
        if resolver != resolver_pin:
            raise RuntimeError('returned resolver differs from retained operator pin')
        note = git('notes','--ref=receipts','show',commit)
        commit_raw = git('cat-file','commit',commit,raw=True)
        (out/'receipt.jsonl').write_text(note+'\n')
        (out/'returned-commit.txt').write_bytes(commit_raw)
        validated = validate_invocation(raw_commit=commit_raw,raw_receipt=note,commit=commit,
            output_tree=tree,program_commit=program,input_commit=inputs,resolver=resolver,
            image=args.expected_image,memo_hit=False,fresh=True)
        if tree != trace['records'][args.step_index]['output_hash']:
            raise RuntimeError('hosted whole filesystem tree differs from genuine recorded step')
        result.update(receipt=validated,recorded_output_tree=tree,resolver=resolver)
        (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result,indent=2))
        return 0

    # This is a SYNTHETIC capability probe, not a full transport export gate:
    # Unforged additionally uploads resolver/seed/meta/worker/argument wrappers.
    # The full closure seam must be integrated before private qualification.
    return export_private(authority,grant,scope,serialize=lambda:None,write=lambda v:v,upload=upload)


if __name__ == '__main__':
    sys.exit(main())
