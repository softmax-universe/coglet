#!/usr/bin/env python3
"""Prepare genuine offline replay/fork prefixes for SYNTHETIC hosted probes.

No remote calls; NOT a private export or qualification command. Use record_oracles
first. Native reexecution verifies the supplied trace before creating any inputs.
"""
import argparse
import hashlib
import json
from pathlib import Path
from prefix_inputs import step_payload
from recorded_closure import require_model_reply_closure
from run import run, save


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for key in ('trace','driver','unforge','output'):
        ap.add_argument('--'+key, type=Path, required=True)
    ap.add_argument('--resolver-store', type=Path, required=True,
                    help='existing local Unforged Git store retaining the independently pinned resolver')
    ap.add_argument('--expected-resolver', required=True)
    args = ap.parse_args()
    driver, unforge = args.driver.resolve(), args.unforge.resolve()
    trace_path = args.trace.resolve()
    trace = json.loads(trace_path.read_bytes())
    # Verifies actual guest execution, not structural import alone.
    verified = json.loads(run(driver, 'record', trace_path))
    if [r['output_hash'] for r in verified['records']] != [r['output_hash'] for r in trace['records']]:
        raise RuntimeError('native oracle differs')
    require_model_reply_closure(trace)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    save(out/'trace.json', trace)
    state = out/'unforged-os'
    run(unforge, 'init', state)
    gitdir = state/'dev'

    def git(*command, data=None):
        return run('git', '--no-replace-objects', f'--git-dir={gitdir}', *command, data=data).decode().strip()

    def tree(files):
        nested = {}
        for path, content in files.items():
            node = nested
            parts = path.split('/')
            for part in parts[:-1]:
                node = node.setdefault(part, {})
            node[parts[-1]] = git('hash-object','-w','--stdin',data=content)
        def build(node):
            rows = []
            for name, value in node.items():
                if isinstance(value, dict):
                    rows.append(f'040000 tree {build(value)}\t{name}\0')
                else:
                    rows.append(f'100644 blob {value}\t{name}\0')
            return git('mktree','-z',data=''.join(rows).encode())
        return build(nested)

    source_pin = run('git','--no-replace-objects',f'--git-dir={args.resolver_store.resolve()}',
                     'rev-parse','refs/heads/resolver').decode().strip()
    if source_pin != args.expected_resolver:
        raise RuntimeError('source resolver differs from independent pin')
    git('fetch','--no-tags',str(args.resolver_store.resolve()),'refs/heads/resolver:refs/heads/resolver')
    if git('rev-parse','refs/heads/resolver') != args.expected_resolver:
        raise RuntimeError('imported resolver differs from independent pin')

    results = []
    for index, record in enumerate(trace['records']):
        payload = step_payload(trace, index)
        save(out/'prefix.json', payload['prefix'])
        files = json.loads(run(driver, 'snapshot', out/'prefix.json'))
        actual = tree({p[1:]: bytes.fromhex(v) for p, v in files.items()})
        if actual != payload['prefix']['final_hash']:
            raise RuntimeError('prefix tree differs from genuine prior state')
        input_tree = tree({**{p[1:]: bytes.fromhex(v) for p, v in files.items()},
                           'proof-input.json': json.dumps(payload).encode()})
        git('update-ref', f'refs/proof/input-{index}', input_tree)
        save(out/f'step-{index}-input.json',payload)
        results.append(dict(step=index, input_tree=input_tree, prior_tree=actual, expected_tree=record['output_hash']))
    save(out/'prepared.json', dict(steps=results, trace_sha256=hashlib.sha256(trace_path.read_bytes()).hexdigest(),
                                  qualification='local inputs only; no hosted or private-export qualification'))
    print(json.dumps(results,indent=2))

if __name__ == '__main__':
    main()
