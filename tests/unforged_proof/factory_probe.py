#!/usr/bin/env python3
"""Actual synthetic hosted factory service path; NOT stage2/production approval.

Keys and grants are owned by this operator testbed process, never the candidate.
The real native suite runs first, then every recorded step through guarded CAOS,
then existing service signing and one-use fixture admission. max-steps=1 is an
honest behavioral rejection control, not a successful policy gate.
"""
import argparse
import gzip
import json
import os
from pathlib import Path
import secrets
import sqlite3
import sys
import tempfile
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT/'crates/coglet-gate/python'),str(ROOT/'sim'),str(ROOT/'sdk/surface-python')]
from coglet_gate import ArtifactStore, Binding, BuildIssuer, GateService, GateVerifier, Suite, TrustKey, Refused, canonical, digest
from coglet_sim.evaluation import NativeTools
from factory_transport import FactoryTransport
from hosted_export import ExportAuthority
from guarded_transport import RecipeAuthority
from run import run


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--synthetic-transport-only',action='store_true')
    for name in ('daemon','proof','driver','bridge','output'):p.add_argument('--'+name,type=Path,required=True)
    for name in ('expected-driver-sha256','expected-bridge-sha256','expected-endpoint','expected-image'):p.add_argument('--'+name,required=True)
    p.add_argument('--max-steps',type=int,default=1)
    args=p.parse_args()
    if not args.synthetic_transport_only:raise Refused('synthetic-only factory probe')
    # Only this committed public synthetic guest is eligible for this testbed CLI.
    fixture=json.loads(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    wasm=bytes.fromhex(fixture['initial_files']['/code/bin/step.wasm'])
    source=run('git','rev-parse','HEAD',cwd=ROOT).decode().strip()
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    worker=(ROOT/'tests/unforged_proof/bridge/src/standard.rs').read_text().split('pub const WORKER: &str = r#"',1)[1].split('"#;',1)[0].encode()
    with tempfile.TemporaryDirectory(prefix='factory-operator-',dir=out) as private:
        private=Path(private)
        store=ArtifactStore(private/'artifacts')
        bk,gk=TrustKey('fixture-build',secrets.token_bytes(32)),TrustKey('fixture-gate',secrets.token_bytes(32))
        source_authority=ExportAuthority(store,TrustKey('fixture-source-export',secrets.token_bytes(32)),set())
        recipe_authority=RecipeAuthority(store,TrustKey('fixture-wire-export',secrets.token_bytes(32)),set())
        transport=FactoryTransport(args.driver.resolve(),args.bridge.resolve(),args.expected_driver_sha256,args.expected_bridge_sha256,
            worker,args.expected_endpoint,args.expected_image,digest(b'coglet synthetic factory operator'),out/'steps',
            source_authority,recipe_authority,source_authority.issue,recipe_authority.issue,'ephemeral-fixture-operator-v1',True)
        service=GateService(store,bk,gk,NativeTools(args.daemon.resolve(),args.proof.resolve()),hosted=transport)
        verifier=GateVerifier(private/'challenge.sqlite',store,bk,gk,tools=service.hosted_tool_identity)
        binding=Binding(fixture['coglet'],'synthetic-stage2-fixture',source,None,'1'*32,digest(wasm))
        suite=Suite('hosted-gridbot-fixture-v1','gridbot',42,args.max_steps,1)
        build=BuildIssuer(store,bk).fixture(binding,wasm,b'committed public synthetic gridbot fixture, not production authority')
        request=verifier.challenge_fixture(binding,suite,build)
        # Public pins only. No keys, MACs, auth tickets or authority stores persist.
        (out/'command.json').write_bytes(canonical(dict(argv=sys.argv,source_head=source,
            daemon_sha256=digest(args.daemon.read_bytes()),proof_sha256=digest(args.proof.read_bytes()),
            driver_sha256=args.expected_driver_sha256,bridge_sha256=args.expected_bridge_sha256,tools=service.hosted_tool_identity)))
        artifact=service.evaluate_hosted(private,request,wasm)
        value=verifier.accept_fixture(request,artifact)
        refused=False
        try:verifier.accept_fixture(request,artifact)
        except Refused:refused=True
        production=False
        try:verifier.verify_production(request,artifact)
        except Refused:production=True
        # Retain the exact public challenge for independent salt verification;
        # challenge MAC is not a key or bearer credential to CAOS/private data.
        (out/'request.json').write_bytes(canonical(request))
        (out/'evaluation.json').write_bytes(canonical(value))
        result=dict(artifact=artifact,steps=len(value['report']['hosted']['steps']),
            behavioral_passed=value['report']['passed'],qualification=value['qualification'],
            second_accept_refused=refused,production_refused=production)
        if not refused or not production:raise Refused('fixture authority widened or replay admitted')
        (out/'result.json').write_bytes(canonical(result))
        print(json.dumps(result))
if __name__=='__main__':main()
