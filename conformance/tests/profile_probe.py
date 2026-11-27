"""Probe peer: reports the trusted provisioning it was launched with, then waits for EOF."""
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wire import encode  # noqa: E402

marker = os.environ.get('COGLET_PROBE_MARKER')
if marker:
    with open(marker, 'a') as out:
        out.write('launched\n')
raw = encode({'profile': os.environ.get('COGLET_CONFORMANCE_CONTRACT_PROFILE', 'unset')})
sys.stdout.buffer.write(len(raw).to_bytes(4, 'big') + raw)
sys.stdout.buffer.flush()
sys.stdin.buffer.read()
