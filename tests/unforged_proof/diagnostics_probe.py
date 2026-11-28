#!/usr/bin/env python3
"""Run the existing STANDARD-worker sidecar route; NOT stage2 qualification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--synthetic-transport-only', action='store_true')
    parser.add_argument('--strict-isolation', action='store_true',
                        help='Exercise existing nested namespace without proxy/fd3; no fallback')
    parser.add_argument('--bridge', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--expected-endpoint', required=True)
    parser.add_argument('--expected-image')
    parser.add_argument('--request', help='Read-only follow-up for a retained synthetic request')
    args = parser.parse_args()
    if not args.synthetic_transport_only:
        raise SystemExit('refused: no full private transport authorization')
    if not os.environ.get('CAOS_IROH_TICKET'):
        raise SystemExit('unavailable: broker ticket missing')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    bridge = args.bridge.resolve()
    if args.request is not None:
        command = [str(bridge), 'synthetic-status', '--synthetic-transport-only',
                   args.expected_endpoint, args.request]
    else:
        if not args.expected_image:
            raise SystemExit('refused: expected image required for execution')
        command = [str(bridge), 'synthetic-isolation' if args.strict_isolation else 'synthetic-diagnostics', '--synthetic-transport-only',
                   args.expected_endpoint, args.expected_image, secrets.token_hex(32)]
    provenance = dict(command=command, bridge_sha256=hashlib.sha256(bridge.read_bytes()).hexdigest(),
                      qualification='diagnostic transport capability only; NOT stage2 PASS')
    (output/'command.json').write_text(json.dumps(provenance, indent=2)+'\n')
    def redact(text):
        for name in ('CAOS_IROH_TICKET', 'UNFORGED_CAOS_URL'):
            value = os.environ.get(name)
            if value:
                text = text.replace(value, '[REDACTED]')
        return re.sub(r'caos://[^\s]+', 'caos://[REDACTED]', text)
    try:
        process = subprocess.run(command, capture_output=True, text=True, timeout=240)
        stdout, stderr, code = process.stdout, process.stderr, process.returncode
    except subprocess.TimeoutExpired as error:
        stdout, stderr, code = error.stdout or b'', error.stderr or b'', 124
        stdout = stdout.decode() if isinstance(stdout, bytes) else stdout
        stderr = stderr.decode() if isinstance(stderr, bytes) else stderr
    (output/'bridge.jsonl').write_text(redact(stdout))
    (output/'bridge-error.txt').write_text(redact(stderr))
    (output/'exit.json').write_text(json.dumps({'returncode': code})+'\n')
    print(json.dumps({'returncode': code, 'output': str(output),
                      'qualification': 'NOT stage2 PASS'}))
    raise SystemExit(code)


if __name__ == '__main__':
    main()
