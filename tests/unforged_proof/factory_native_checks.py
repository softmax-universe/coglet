#!/usr/bin/env python3
"""Explicit real native factory archive compatibility check; never a hosted PASS."""
import argparse
import gzip
import json
import os
from pathlib import Path
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(ROOT/'sim'),str(ROOT/'crates/coglet-gate/python'),str(ROOT/'sdk/surface-python')]
from coglet_sim.evaluation import NativeTools, evaluate
from recorded_closure import require_model_reply_closure
from prefix_inputs import step_payload
from standard_step import state_files, snapshot_tree


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('daemon','proof','driver','output'):p.add_argument('--'+name,type=Path,required=True)
    args=p.parse_args()
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    fixture=json.loads(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    wasm=bytes.fromhex(fixture['initial_files']['/code/bin/step.wasm'])
    archive=out/'archive.json'
    report=evaluate(out,NativeTools(args.daemon.resolve(),args.proof.resolve()),wasm,
                    world='gridbot',seed=42,max_steps=1,minimum_actions=1,coglet=fixture['coglet'],archive=archive)
    trace=json.loads(archive.read_bytes())
    require_model_reply_closure(trace)
    result=subprocess.run([str(args.driver.resolve()),'record',str(archive)],capture_output=True,
        env={k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')},timeout=120)
    if result.returncode:raise RuntimeError('native archive reexecution failed: '+result.stderr.decode(errors='replace'))
    verified=json.loads(result.stdout)
    if [r['output_hash'] for r in verified['records']] != [r['output_hash'] for r in trace['records']]:
        raise RuntimeError('native archive trees differ')
    for i,record in enumerate(trace['records']):
        snapshot=trace['initial_files'] if i==0 else trace['records'][i-1]['output_files']
        # Current boundary is an input to runtime.step, not pre-applied to this snapshot.
        payload=step_payload(trace,i)
        files=state_files(snapshot,payload)
        if snapshot_tree({p:v for p,v in files.items() if p!='proof-input.json'}) != payload['prefix']['final_hash']:
            raise RuntimeError('prepared prior tree differs')
    (out/'native-report.json').write_text(json.dumps(report,sort_keys=True))
    summary=dict(steps=len(trace['records']),trees=[r['output_hash'] for r in trace['records']],
                 behavioral_passed=report['passed'],qualification='real native factory archive; NOT hosted')
    (out/'result.json').write_text(json.dumps(summary,sort_keys=True))
    print(json.dumps(summary))
if __name__=='__main__':main()
