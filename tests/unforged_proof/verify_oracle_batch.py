#!/usr/bin/env python3
"""Independently re-read synthetic batch receipts; preserve compact raw evidence."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import zipfile
from hosted_receipts import validate_invocation


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--batch',type=Path,required=True)
    ap.add_argument('--output',type=Path,required=True)
    args = ap.parse_args()
    batch = args.batch.resolve()
    summary = json.loads((batch/'summary.json').read_bytes())
    counts = Counter(row['path'] for row in summary['steps'])
    if counts != {'replay':9,'fork':9} or summary['verified_count'] != 18:
        raise RuntimeError('incomplete batch')
    evidence_files = [batch/'summary.json']
    rows = []
    for row in summary['steps']:
        entry = batch/f"{row['path']}-{row['step']}"
        result = json.loads((entry/'result.json').read_bytes())
        receipt = validate_invocation(raw_commit=(entry/'returned-commit.txt').read_bytes(),
            raw_receipt=(entry/'receipt.jsonl').read_text(),commit=row['receipt']['commit'],
            output_tree=row['expected_tree'],program_commit=row['program'],input_commit=row['input'],
            resolver=result['resolver'],image=row['receipt']['worker_image'],memo_hit=False,fresh=True)
        if receipt != row['receipt'] or result['recorded_output_tree'] != row['expected_tree']:
            raise RuntimeError('readback mismatch')
        rows.append(dict(path=row['path'],step=row['step'],commit=receipt['commit']))
        evidence_files += [entry/name for name in ('result.json','receipt.jsonl','returned-commit.txt','discovery.json')]
    if len({r['commit'] for r in rows}) != len(rows):
        raise RuntimeError('ambiguous duplicate returned commits')
    output = args.output.resolve()
    with zipfile.ZipFile(output,'x',compression=zipfile.ZIP_DEFLATED) as archive:
        for path in evidence_files:
            archive.write(path,path.relative_to(batch))
    print(json.dumps(dict(verified_counts=counts,source_head=summary['source_head'],
        artifact=str(output),sha256=hashlib.sha256(output.read_bytes()).hexdigest(),bytes=output.stat().st_size,
        qualification='18 synthetic hosted step receipts read back; NOT stage2 PASS'),indent=2))

if __name__ == '__main__':
    main()
