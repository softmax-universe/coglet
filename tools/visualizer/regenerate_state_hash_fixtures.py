#!/usr/bin/env python3
"""Recapture synthetic CLI + Gridbot traces after ADR-0035.

Build first (debug binaries work too): coglet, cogletd, coglet-sandbox-proof,
coglet-visualizer-native; compile/qualify the Gridbot pilot as in sim/check-native.sh.
Run with the sim environment: uv run --project sim --locked python
  tools/visualizer/regenerate_state_hash_fixtures.py --bin-dir target/debug
  --pilot coglets/gridbot-pilot/target/wasm32-unknown-unknown/release/coglet_gridbot_pilot.wasm

No old trace is rehashed or converted. All records come from fresh real WAMR
execution; the native helper exports only this stopped synthetic test WAL.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / 'tools/visualizer/tests/fixtures'
NATIVE = ROOT / 'tools/visualizer/native/fixtures'
DEFINITION = 'git-sha256-tree-id (ADR-0035)'


def run(*args):
    return subprocess.run([str(a) for a in args], check=True, capture_output=True).stdout


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def projection(records):
    keys = ('version', 'trace_id', 'coglet', 'step', 'logical_time', 'status',
            'output_hash', 'record_hash', 'events', 'engine')
    return {'records': [dict({k: r[k] for k in keys}, output_files={
        p: b for p, b in r['output_files'].items() if p.startswith('/data/')}) for r in records]}


def capture_proof(args):
    capture = Path(os.environ['COGLET_FIXTURE_CAPTURE'])
    binaries = Path(os.environ['COGLET_FIXTURE_BIN'])
    run(binaries / 'coglet-sandbox-proof', *args)
    run(binaries / 'coglet-visualizer-native', 'export', args[0], args[1], capture / 'trace.json')
    (capture / 'proof.json').write_bytes(Path(args[2]).read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, required=True)
    parser.add_argument('--pilot', type=Path, required=True)
    opts = parser.parse_args()
    binaries, pilot = opts.bin_dir.resolve(), opts.pilot.resolve()
    revision = run('git', 'rev-parse', 'HEAD').decode().strip()
    # Only fixture outputs may be dirty: provenance names the executed source.
    for path in ('crates', 'sdk', 'sim', 'coglets', 'tools/visualizer/native/src', 'Cargo.lock'):
        if run('git', 'diff', 'HEAD', '--', path):
            raise RuntimeError(f'commit producer source before capture: {path}')
    with tempfile.TemporaryDirectory(prefix='state-fixtures-', dir=os.environ['TMPDIR']) as temp:
        tmp = Path(temp)
        run('cargo', 'run', '--locked', '-p', 'coglet-cli', '--example', 'visualizer_fixture_guest', '--', tmp / 'log.wasm')
        coglet = 'coglet:' + 'c' * 64
        manifest = dict(version='1.0', id=coglet, subscriptions=[], required_capabilities=[], exclusive_outputs=[],
                        budgets=dict(compute_mode='wamr-2.4.3-fast-cumulative-v1', compute=100000,
                                     memory_pages=2, host_bytes=1000000, wall_time_us=2000000, max_events=64))
        write(tmp / 'manifest.json', manifest)
        (tmp / 'recipe').write_text('synthetic WAT visualizer fixture guest')
        (tmp / 'policy').write_text('unsigned local sandbox; no dispatch')
        run(binaries / 'coglet', 'build', '--wasm', tmp / 'log.wasm', '--manifest', tmp / 'manifest.json',
            '--source-commit', revision, '--recipe', tmp / 'recipe', '--policy', tmp / 'policy', '--output', tmp / 'package')
        batches = [[dict(kind='runtime.debug_step', source=coglet, seq=i, logical_time=i,
                         correlation_id='1'*32, payload={})] for i in range(2)]
        write(tmp / 'events.json', batches)
        run(binaries / 'coglet', 'run', tmp / 'package', '--events', tmp / 'events.json',
            '--output', tmp / 'cli-trace.json', '--cogletd', binaries / 'cogletd')
        run(binaries / 'coglet', 'replay', tmp / 'cli-trace.json', '--package', tmp / 'package')
        raw = (tmp / 'cli-trace.json').read_bytes()
        trace = json.loads(raw)
        display = projection(trace['records'])
        write(FIXTURES / 'native-cli-wamr-trace.json', display)
        write(FIXTURES / 'native-cli-wamr-trace.provenance.json', dict(
            producer_sha=revision, engine='wamr', state_hash_definition=DEFINITION,
            producer_command='tools/visualizer/regenerate_state_hash_fixtures.py (real coglet build/run/replay)',
            full_trace_sha256=sha(raw), records_only_sha256=sha((FIXTURES / 'native-cli-wamr-trace.json').read_bytes()),
            final_hash=trace['final_hash'], omitted_from_tree=['initial_files', 'blobs', '/code'],
            note='Synthetic test guest; projection copies observed hashes and /data verbatim.'))
        wrapper = tmp / 'proof-wrapper'
        import shlex
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.join([sys.executable, str(Path(__file__).resolve()), 'capture-proof']) + ' "$@"\n')
        wrapper.chmod(0o700)
        os.environ['COGLET_FIXTURE_CAPTURE'] = str(tmp)
        os.environ['COGLET_FIXTURE_BIN'] = str(binaries)
        sys.path[:0] = [str(ROOT / 'sim'), str(ROOT / 'sdk/surface-python')]
        from coglet_sim.evaluation import NativeTools, evaluate
        report = evaluate(tmp, NativeTools(binaries / 'cogletd', wrapper), pilot.read_bytes(), world='gridbot', seed=42, max_steps=16)
        assert report['passed'], report
        raw = (tmp / 'trace.json').read_bytes()
        trace = json.loads(raw)
        # Validate against independently executed proof outputs before publication.
        proof_raw = (tmp / 'proof.json').read_bytes()
        proof = json.loads(proof_raw)
        assert [r['output_hash'] for r in trace['records']] == proof['state_hashes']
        display = projection(trace['records'])
        common = dict(producer_sha=revision, engine='wamr', state_hash_definition=DEFINITION,
                      producer_command='tools/visualizer/regenerate_state_hash_fixtures.py (real evaluate + stopped-WAL export)',
                      full_trace_sha256=sha(raw), proof_output_sha256=sha(proof_raw),
                      independent_guest_reexecution=True, changed_seed_rejected=proof['changed_seed_rejected'],
                      changed_guest_rejected=proof['changed_guest_rejected'],
                      world='gridbot-v1', seed=42, config=dict(width=8, height=8, max_steps=16),
                      actions=['east', 'east', 'south', 'south'],
                      display_omits=['initial_files', 'blobs', '/code', 'calls', 'usage'],
                      steps=[dict(output_hash=r['output_hash'], inbox_bytes=len(bytes.fromhex(r['output_files'].get('/data/inbox.jsonl', '')))) for r in trace['records']])
        for name in ('native-m2-gridbot-trace', 'world-m2-gridbot-trace'):
            write(FIXTURES / (name + '.json'), display)
            write(FIXTURES / (name + '.provenance.json'), dict(common, records_only_sha256=sha((FIXTURES / (name + '.json')).read_bytes())))
        compressed = gzip.compress(raw, mtime=0)
        (NATIVE / 'm2-gridbot-trace.json.gz').write_bytes(compressed)
        write(NATIVE / 'provenance.json', dict(common, fixture_sha256=sha(compressed),
              canonical_trace_sha256=sha(raw), canonical_trace_bytes=len(raw), final_hash=trace['final_hash'], records=len(trace['records'])))
        print('Recaptured CLI and Gridbot fixture traces with real WAMR execution and independent replay.')


if __name__ == '__main__':
    if sys.argv[1:2] == ['capture-proof']:
        capture_proof(sys.argv[2:])
    else:
        main()
