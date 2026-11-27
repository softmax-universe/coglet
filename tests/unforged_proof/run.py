#!/usr/bin/env python3
"""Opt-in real WAMR / local Merge Stream proof; stdlib only."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def run(*args, data=None, cwd=None):
    p = subprocess.run([str(a) for a in args], input=data, capture_output=True, cwd=cwd, timeout=300)
    if p.returncode:
        raise RuntimeError(f'{args}: exit {p.returncode}\n{p.stderr.decode(errors="replace")}\n{p.stdout.decode(errors="replace")}')
    return p.stdout


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def prefix(trace, n):
    value = copy.deepcopy(trace)
    value['records'] = value['records'][:n]
    value['final_hash'] = value['records'][-1]['output_hash'] if n else value['initial_hash']
    value['trailing_boundary'] = None
    # Only code/manifest blobs are needed by ABI 1.0. Never supply current or
    # future guest outputs to the job that must independently compute them.
    value['blobs'] = {h: value['blobs'][h] for h in (value['code_hash'], value['manifest_hash'])}
    return value


def receipt(text):
    match = re.search(r'tree=([0-9a-f]{64}) memo_hit=(true|false)', text)
    if not match:
        raise ValueError(f'Merge Stream receipt missing tree/memo_hit: {text}')
    return match[1], match[2] == 'true'


def comparison(step, expected, cold, warm):
    cold_tree, cold_hit = receipt(cold)
    warm_tree, warm_hit = receipt(warm)
    return {'step': step, 'recorded_state_hash': expected, 'uncached_tree': cold_tree,
            'uncached_memo_hit': cold_hit, 'cached_tree': warm_tree, 'cache_hit': warm_hit,
            'pass': expected == cold_tree == warm_tree and not cold_hit and warm_hit}


def require(condition, diagnostic):
    if not condition:
        raise RuntimeError(str(diagnostic))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--unforge', required=True, type=Path)
    ap.add_argument('--output', required=True, type=Path, help='new evidence directory')
    ap.add_argument('--bin-dir', type=Path, default=ROOT / 'target/debug')
    opts = ap.parse_args()
    out = opts.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    unforge = opts.unforge.resolve()
    bins = opts.bin_dir.resolve()
    driver = bins / 'coglet-unforged-proof'
    cli = bins / 'coglet'
    daemon = bins / 'cogletd'
    manifest = {'version':'1.0','id':'coglet:' + '09'*32,
                'subscriptions':[], 'required_capabilities':[], 'exclusive_outputs':[],
                'budgets':{'compute_mode':'wamr-2.4.3-fast-cumulative-v1', 'compute':10000000,
                           'memory_pages':256,'host_bytes':1000000,'wall_time_us':10000000,'max_events':64}}
    save(out / 'manifest.json', manifest)
    (out / 'recipe').write_text('cargo rustc --locked --manifest-path coglets/counter/Cargo.toml --target wasm32-wasip1 --release --lib -- -C link-arg=--max-memory=16777216\n')
    (out / 'policy').write_text('unsigned, local synthetic counter only; no dispatch\n')
    run(cli, 'build', '--wasm', ROOT / 'coglets/counter/target/wasm32-wasip1/release/coglet_counter.wasm',
        '--manifest', out/'manifest.json', '--source-commit', run('git','rev-parse','HEAD',cwd=ROOT).decode().strip(),
        '--recipe',out/'recipe','--policy',out/'policy','--output',out/'package')
    run(driver,'prepare',out/'package',out/'hot-job.json')
    job = json.loads((out/'hot-job.json').read_text())
    raw = run(daemon,'--sandbox-stdio',data=json.dumps(job).encode())
    (out/'trace.json').write_bytes(raw)
    trace = json.loads(raw)
    require(len(trace['records']) >= 5, 'proof requires at least five steps')
    require(all(r['status']=='committed' for r in trace['records']), trace['records'])
    osroot = out / 'unforged-os'
    run(unforge,'init',osroot)
    gitdir = osroot/'dev'

    def git(*args, data=None):
        return run('git',f'--git-dir={gitdir}',*args,data=data).decode().strip()

    def tree(files):
        nested = {}
        for path, (mode, content) in files.items():
            node = nested
            parts = path.split('/')
            for part in parts[:-1]:
                node = node.setdefault(part,{})
            node[parts[-1]] = (mode, git('hash-object','-w','--stdin',data=content))
        def build(node):
            rows = []
            for name, item in node.items():
                if isinstance(item,dict):
                    mode, kind, oid = '040000','tree',build(item)
                else:
                    mode, oid = item
                    kind = 'blob'
                rows.append(f'{mode} {kind} {oid}\t{name}\0')
            return git('mktree','-z',data=''.join(rows).encode())
        return build(nested)

    program = tree({'proof-driver':('100755', driver.read_bytes())})
    result = {'git_version':run('git','--version').decode().strip(),
              'unforge_binary_sha256':hashlib.sha256(unforge.read_bytes()).hexdigest(),
              'driver_binary_sha256':hashlib.sha256(driver.read_bytes()).hexdigest(),
              'coglet_head':run('git','rev-parse','HEAD',cwd=ROOT).decode().strip(),
              'initial_hash':trace['initial_hash'],'final_hash':trace['final_hash'], 'steps':[]}
    save(out/'results.json',result)
    for n, record in enumerate(trace['records']):
        pre = prefix(trace,n)
        save(out/'prefix.json', pre)
        prior_files = json.loads(run(driver,'snapshot',out/'prefix.json'))
        require(bytes.fromhex(prior_files.get('/data/count', '30')).decode() == str(n), 'counter prior count mismatch')
        prior_tree = tree({p[1:]:('100644',bytes.fromhex(b)) for p,b in prior_files.items()})
        require(prior_tree == pre['final_hash'], (prior_tree,pre['final_hash']))
        # Whitelist actual recorded inputs, never the expected output/delta.
        inp = {k:record[k] for k in ('trace_id','coglet','step','logical_time','seed','previous','budgets','grants','events','boundary')}
        payload = {'prefix':pre,'setup':job['setup'],'input':inp}
        input_tree = tree({**{p[1:]:('100644',bytes.fromhex(b)) for p,b in prior_files.items()},
                           'proof-input.json':('100644',json.dumps(payload).encode())})
        git('update-ref', f'refs/proof/input-{n}', input_tree)
        save(out/f'step-{n}-input.json', payload)
        # No no-cache flag exists on current stream exec. Evict the entire
        # local memoization notes ref before EACH forced execution. The second
        # call reuses the resulting entry without eviction (cache enabled).
        git('update-ref','-d','refs/notes/memoization')
        cold = run(unforge,'--root',osroot,'stream','exec',program,input_tree,'--bin','proof-driver').decode().strip()
        (out/f'step-{n}-cold.txt').write_text(cold+'\n')
        receipt(cold)  # Refuse incomplete/ambiguous job output before repeating.
        warm = run(unforge,'--root',osroot,'stream','exec',program,input_tree,'--bin','proof-driver').decode().strip()
        (out/f'step-{n}-warm.txt').write_text(warm+'\n')
        row = comparison(n, record['output_hash'], cold, warm)
        for label, text in [('cold',cold),('warm',warm)]:
            commit_match = re.search(r'CONFLUENCE ([0-9a-f]{64}) ',text)
            if commit_match is None:
                raise ValueError(f'Missing result commit: {text}')
            commit = commit_match[1]
            note = git('notes','--ref=provenance','show',commit)
            (out/f'step-{n}-{label}-provenance.txt').write_text(note+'\n')
            require(f'memo_hit={str(label == "warm").lower()}' in note, note)
        save(out/'prefix.json',prefix(trace,n+1))
        after_files = json.loads(run(driver,'snapshot',out/'prefix.json'))
        require(bytes.fromhex(after_files['/data/count']).decode() == str(n+1), 'counter output count mismatch')
        result['steps'].append(row)
        save(out/'results.json',result)
        print(json.dumps(row),flush=True)
        require(row['pass'], row)
    require(len(result['steps']) == len(trace['records']), 'missing results')
    print('PASS',out/'results.json')


if __name__ == '__main__':
    main()
