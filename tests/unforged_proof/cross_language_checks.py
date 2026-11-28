#!/usr/bin/env python3
"""Explicit offline Rust->Python complete guarded contract, not hosted proof."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore,TrustKey
from guarded_transport import ApprovalSession,RecipeAuthority,recipe_for,scope_for,strict_json
from run import ROOT

def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--target-dir',type=Path,required=True)
    args = ap.parse_args()
    with tempfile.TemporaryDirectory(prefix='cross-language-') as tmp:
        tmp = Path(tmp)
        env = dict(os.environ,COGLET_CROSS_LANGUAGE_OUTPUT=str(tmp/'transcripts.json'))
        subprocess.run(['cargo','+1.100.0','test','--locked','--offline','--manifest-path',str(ROOT/'tests/unforged_proof/bridge/Cargo.toml'),'--target-dir',str(args.target_dir.resolve()),'-j2','--','--ignored','--exact','guarded::tests::cross_language_contract_fixture','--nocapture'],check=True,env=env,timeout=120)
        transcripts = strict_json((tmp/'transcripts.json').read_bytes())
        source = (ROOT/'tests/unforged_proof/bridge/src/standard.rs').read_text()
        worker = source.split('pub const WORKER: &str = r#"',1)[1].split('"#;',1)[0].encode()
        files = {'sub/a.txt':b'alpha','b.txt':b''}
        salt,image,endpoint = 'c'*64,'native@sha256:'+'a'*64,'d'*64
        recipe = recipe_for(files,['empty','empty/nested','sub'],driver=b'\x7fELFdriver',worker=worker,salt=salt,image=image)
        scope = scope_for(recipe,sender='e'*64,recipient=endpoint,coglet='coglet:'+'f'*64)
        blobs = {'args/state/'+p:b for p,b in files.items()}
        blobs.update({'args/driver':b'\x7fELFdriver','args/worker1':worker,'args/salt':salt.encode(),'args/required-native-worker-image':image.encode(),'args/required-pool':b'unforged'})
        authority = RecipeAuthority(ArtifactStore(tmp/'authority'),TrustKey('offline-operator',b'x'*32),set())
        grant = authority.issue(scope)
        counts = []
        if len(transcripts) != 2: raise RuntimeError('two actual Rust runs required')
        for rows in transcripts:
            session = ApprovalSession(authority,grant,scope,retained_blobs=blobs)
            for row in rows:
                if row['phase'] == 'authorize': session.approve(row)
                else: session.observe(row)
            session.finish()
            counts.append(Counter(r['path'] for r in rows if r.get('operation') == 'request' and r.get('method') == 'POST'))
        if counts[0]['/object/'] <= 0 or counts[1]['/object/'] != 0: raise RuntimeError('cold/miss and warm/hit branches not both exercised')
        print(json.dumps(dict(offline_rust_python_contract='PASS',runs=len(transcripts),object_posts=[c['/object/'] for c in counts],qualification='actual Rust events, fake network only; NOT hosted proof')))
if __name__ == '__main__': main()
