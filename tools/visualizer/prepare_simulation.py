"""Prepare a private world-replay envelope from an explicitly authorized M2 export.

This does not invent ABI fields, authenticate an archive, or verify guest code.
"""
import argparse
import json
import os
from pathlib import Path
import stat
import tempfile

from simulation import replay
from server import MAX_BYTES, project


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--trace', type=Path, required=True)
    parser.add_argument('--world', choices=['gridbot-v1', 'cartpole-v1'], required=True)
    parser.add_argument('--seed', type=int, required=True)
    parser.add_argument('--config', required=True, help='Explicit world configuration JSON')
    parser.add_argument('--actions', required=True, help='Recorded ordered world actions as JSON')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    fd = os.open(args.trace, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            raise ValueError('Trace must be a regular file')
        raw = source.read(MAX_BYTES + 1)
    if len(raw) > MAX_BYTES:
        raise ValueError('Trace size limit')
    trace = json.loads(raw)
    archive = dict(format='coglet-world-replay-v1', world=args.world, seed=args.seed,
                   config=json.loads(args.config), actions=json.loads(args.actions), records=trace['records'])
    replay(archive)
    for record in archive['records']:
        project(record)
    body = json.dumps(archive).encode()
    if len(body) > MAX_BYTES - 1024:
        raise ValueError('Envelope size limit')
    with tempfile.NamedTemporaryFile(dir=args.output.parent) as target:
        target.write(body)
        target.flush()
        os.fsync(target.fileno())
        os.link(target.name, args.output)  # Atomic no-clobber publication, mode 0600.
    print('Prepared world-model-only archive; guest hashes remain unverified.')


if __name__ == '__main__':
    main()
