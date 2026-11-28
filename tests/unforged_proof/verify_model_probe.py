#!/usr/bin/env python3
"""Read back both synthetic hosted model receipts; NOT stage2 qualification."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile
from hosted_receipts import validate_invocation
from recorded_closure import require_model_reply_closure


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--workspace', type=Path, required=True)
    ap.add_argument('--source-head', required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    root = args.workspace.resolve()
    trace_path = root/'prepared-model/trace.json'
    trace = json.loads(trace_path.read_bytes())
    closure = require_model_reply_closure(trace)
    if closure != dict(requests=1, replies=1) or len(trace['records']) != 2:
        raise RuntimeError('expected both real model fixture steps')
    files = [trace_path]
    rows = []
    for index, record in enumerate(trace['records']):
        entry = root/f'hosted-model-{index}'
        result = json.loads((entry/'result.json').read_bytes())
        receipt = validate_invocation(raw_commit=(entry/'returned-commit.txt').read_bytes(),
            raw_receipt=(entry/'receipt.jsonl').read_text(), commit=result['receipt']['commit'],
            output_tree=record['output_hash'], program_commit=result['program'], input_commit=result['input'],
            resolver=result['resolver'], image=result['receipt']['worker_image'], memo_hit=False, fresh=True)
        if receipt != result['receipt'] or result['recorded_output_tree'] != record['output_hash']:
            raise RuntimeError('model receipt readback mismatch')
        rows.append(dict(step=index, expected_tree=record['output_hash'], receipt=receipt,
                         driver_sha256=result['driver_sha256']))
        files.extend(entry/name for name in ('result.json','receipt.jsonl','returned-commit.txt','discovery.json'))
    if len({row['receipt']['commit'] for row in rows}) != 2:
        raise RuntimeError('ambiguous repeated commit')
    summary = dict(source_head=args.source_head, closure=closure, steps=rows,
                   qualification='synthetic hosted model request/reply capability; NOT stage2 PASS')
    with zipfile.ZipFile(args.output, 'x', compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr('summary.json', json.dumps(summary, indent=2)+'\n')
        for path in files:
            archive.write(path, path.relative_to(root))
    print(json.dumps(dict(summary=summary, sha256=hashlib.sha256(args.output.read_bytes()).hexdigest(),
                          bytes=args.output.stat().st_size), indent=2))


if __name__ == '__main__':
    main()
