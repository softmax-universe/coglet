#!/usr/bin/env python3
"""One retained Unforged client: bypass/default/default synthetic cache control.

Use ONLY an existing synthetic capability probe's disposable OS. This command
retains the incomplete transport-closure restriction; it cannot authorize private
exports, establish sandbox isolation, or qualify image upgrades/factory gates.
"""
import argparse
import json
import os
import re
from pathlib import Path
import subprocess

from hosted_receipts import Refused, validate_invocation


def verify_cache_sequence(raw, *, program, inputs, resolver, endpoint, image, tree):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise Refused('duplicate bridge observation field')
            result[key] = value
        return result
    try:
        events = [json.loads(line, object_pairs_hook=unique) for line in raw.splitlines() if line.strip()]
        if len(events) != 4:
            raise Refused('exactly four ordered bridge observations required')
        discovery, *runs = events
        if discovery.get('same_client') is not True or discovery.get('endpoint_identity') != endpoint:
            raise Refused('retained authenticated client identity mismatch')
        images = discovery.get('native_images')
        if not isinstance(images, list) or len(images) != 2 or any(i is not None and i != image for i in images):
            raise Refused('discovered image differs from operator pin')
        if [run.get('label') for run in runs] != ['cold', 'default-1', 'default-2']:
            raise Refused('ordered unique cache control labels required')
        checked = []
        for run in runs:
            hit = run['memo_hit']
            if (type(hit) is not bool or run['tree'] != tree or run['resolver'] != resolver
                    or (run['label'] == 'cold' and hit)
                    or (run['label'] == 'default-2' and not hit)):
                raise Refused('cold/warm control disposition or tree mismatch')
            receipt = validate_invocation(raw_commit=run['raw_commit'].encode(),
                raw_receipt=run['raw_receipt'], commit=run['commit'], output_tree=tree,
                program_commit=program, input_commit=inputs, resolver=resolver,
                image=image, memo_hit=hit, fresh=not hit)
            checked.append(dict(label=run['label'], commit=run['commit'], tree=tree, receipt=receipt))
        return checked
    except (KeyError, TypeError, ValueError) as error:
        if isinstance(error, Refused):
            raise
        raise Refused('invalid cache control transcript') from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--synthetic-transport-only', action='store_true')
    for name in ('bridge', 'probe', 'output'):
        parser.add_argument('--'+name, type=Path, required=True)
    for name in ('expected-endpoint', 'expected-image', 'expected-resolver'):
        parser.add_argument('--'+name, required=True)
    args = parser.parse_args()
    if not args.synthetic_transport_only:
        raise SystemExit('refused: full transport-closure authorization unavailable; synthetic-only')
    if not os.environ.get('CAOS_IROH_TICKET'):
        raise SystemExit('broker ticket missing; unavailable')
    probe = args.probe.resolve()
    old = json.loads((probe/'result.json').read_bytes())
    if old['resolver'] != args.expected_resolver:
        raise Refused('retained resolver differs from operator pin')
    # Existing result is an operator-selected SYNTHETIC probe, not authorization.
    # Reuse the actual program/input objects; no new fixture or expected-output executor.
    out = args.output.resolve()
    out.mkdir(exist_ok=False, parents=True)
    command = [str(args.bridge.resolve()), 'synthetic-cache-controls', '--synthetic-transport-only',
               str(probe/'os'), old['program'], old['input'], args.expected_resolver, args.expected_endpoint]
    process = subprocess.run(command, capture_output=True, text=True, timeout=600)
    # Redact defensively even though the bridge's own errors are fixed strings;
    # dependencies may add diagnostics in future. Never persist broker values.
    def redact(text):
        for key in ('CAOS_IROH_TICKET', 'UNFORGED_CAOS_URL'):
            value = os.environ.get(key)
            if value:
                text = text.replace(value, '[REDACTED]')
        return re.sub(r'caos://[^\s]+', 'caos://[REDACTED]', text)
    stdout, stderr = redact(process.stdout), redact(process.stderr)
    (out/'bridge.jsonl').write_text(stdout)
    (out/'bridge-error.txt').write_text(stderr)
    if process.returncode:
        raise Refused('bridge sequence failed; raw partial observations retained')
    checked = verify_cache_sequence(stdout, program=old['program'], inputs=old['input'],
        resolver=args.expected_resolver, endpoint=args.expected_endpoint,
        image=args.expected_image, tree=old['recorded_output_tree'])
    result = dict(qualification='synthetic cache control ONLY; NOT stage2 PASS',
                  program=old['program'], input=old['input'], resolver=args.expected_resolver,
                  endpoint=args.expected_endpoint, image=args.expected_image, observations=checked)
    (out/'result.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
