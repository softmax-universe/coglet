#!/usr/bin/env python3
"""Independent local object readback of actual synthetic cache-control evidence.

This does not reauthenticate a CAOS server; the trusted retained client made those
observations. Changed-pin negatives below reject captured evidence, not an actual
worker/resolver deployment or upgrade. Never label this stage-2 qualification.
"""
import argparse
from datetime import datetime, timezone
from hashlib import sha256
import json
from pathlib import Path
import subprocess
import zipfile

from cache_controls import verify_cache_sequence
from hosted_receipts import Refused


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('controls', 'probe', 'bridge', 'driver', 'output'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--source-head', required=True)
    args = parser.parse_args()
    controls = args.controls.resolve()
    expected = json.loads((controls/'result.json').read_bytes())
    transcript = (controls/'bridge.jsonl').read_text()
    pins = dict(program=expected['program'], inputs=expected['input'],
                resolver=expected['resolver'], endpoint=expected['endpoint'],
                image=expected['image'], tree=expected['observations'][0]['tree'])
    verified = verify_cache_sequence(transcript, **pins)
    events = [json.loads(line) for line in transcript.splitlines() if line.strip()]
    files = {name:(controls/name).read_bytes() for name in ('result.json', 'bridge.jsonl', 'bridge-error.txt')}
    for event in events[1:]:
        for filename, command in [('receipt.jsonl', ['notes','--ref=receipts','show',event['commit']]),
                                  ('commit.txt', ['cat-file','commit',event['commit']])]:
            actual = subprocess.run(['git','--no-replace-objects',
                                     '--git-dir='+str(args.probe.resolve()/'os/dev'), *command],
                                    check=True, capture_output=True).stdout
            field = 'raw_receipt' if filename == 'receipt.jsonl' else 'raw_commit'
            if actual.decode() != event[field]:
                raise Refused('captured observation differs from actual Git readback')
            files[event['label']+'/'+filename] = actual
    negative_results = []
    for name, raw, changed in [
        ('changed-expected-image', transcript, dict(image='native@sha256:'+'0'*64)),
        ('changed-expected-resolver', transcript, dict(resolver='0'*64)),
        ('missing-receipt', None, {}),
        ('ambiguous-receipt', None, {}),
        ('default-2-miss', None, {}),
    ]:
        if raw is None:
            modified = json.loads(json.dumps(events))
            if name == 'missing-receipt':
                modified[1]['raw_receipt'] = ''
            elif name == 'ambiguous-receipt':
                modified[1]['raw_receipt'] *= 2
            else:
                modified[-1]['memo_hit'] = False
            raw = '\n'.join(json.dumps(event) for event in modified)
        try:
            verify_cache_sequence(raw, **{**pins, **changed})
        except Refused:
            negative_results.append(dict(name=name, refused=True,
                scope='local revalidation of actual captured observations, not hosted upgrade'))
        else:
            raise RuntimeError('negative unexpectedly accepted: '+name)
    summary = dict(qualification='synthetic retained-client cache controls ONLY; NOT stage2 PASS',
        observed_at=datetime.now(timezone.utc).isoformat(), source_head=args.source_head,
        driver_source_head='c6ff3b3ac77dcdfecbca53080942abc11d95f882',
        bridge_sha256=sha256(args.bridge.read_bytes()).hexdigest(),
        driver_sha256=sha256(args.driver.read_bytes()).hexdigest(),
        observations=verified, negatives=negative_results,
        limits=['No full transport export authorization', 'No worker sandbox diagnostics/canaries',
                'No actual image/resolver upgrade', 'No factory qualification', 'No stage2 PASS'])
    files['summary.json'] = (json.dumps(summary, indent=2)+'\n').encode()
    with zipfile.ZipFile(args.output, 'x', compression=zipfile.ZIP_DEFLATED) as archive:
        for name, raw in files.items():
            archive.writestr(name, raw)
    print(json.dumps(dict(output=str(args.output.resolve()), bytes=args.output.stat().st_size,
                         sha256=sha256(args.output.read_bytes()).hexdigest(),
                         actual_observations=len(verified), local_negatives=len(negative_results)), indent=2))


if __name__ == '__main__':
    main()
