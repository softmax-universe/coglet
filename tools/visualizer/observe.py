"""Opt-in local observer for cogletd --mock-stdio (reference engine only).

Passes stdin/stdout bytes unchanged; writes only actual journal replies to a new
private JSONL export. This does not add a remote debug grant or run Wasm.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading

import cbor2
from server import MAX_BYTES, MAX_RECORD_BYTES


def observe(binary, path):
    # Never clobber an existing operator export or follow a symlink.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb', buffering=0) as export:
        child = subprocess.Popen([binary, '--mock-stdio'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        assert child.stdin is not None and child.stdout is not None
        def stop(signum, frame):
            child.terminate()
        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGINT, stop)
        def forward():
            try:
                while chunk := os.read(sys.stdin.fileno(), 65536):
                    child.stdin.write(chunk)
                    child.stdin.flush()
            except (BrokenPipeError, OSError):
                pass
            finally:
                child.stdin.close()
        threading.Thread(target=forward, daemon=True).start()
        written = 0
        def exact(size):
            data = child.stdout.read(size)
            if len(data) != size:
                raise ValueError('Truncated runtime response')
            return data
        try:
            while header := child.stdout.read(4):
                if len(header) != 4:
                    raise ValueError('Truncated runtime frame header')
                size = int.from_bytes(header, 'big')
                if not 0 < size <= MAX_RECORD_BYTES:
                    raise ValueError('Runtime frame size limit')
                raw = exact(size)
                reply = cbor2.loads(raw)
                if isinstance(reply, dict) and isinstance(reply.get('journal'), dict):
                    line = json.dumps(reply['journal'], allow_nan=False).encode() + b'\n'
                    if len(line) > MAX_RECORD_BYTES or written + len(line) > MAX_BYTES:
                        raise ValueError('Export size limit; restart with a new export')
                    export.write(line)
                    written += len(line)
                sys.stdout.buffer.write(header + raw)
                sys.stdout.buffer.flush()
            return child.wait(timeout=5)
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill(); child.wait()
            child.stdout.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--records', type=Path, required=True)
    parser.add_argument('--cogletd', required=True, help='Explicit path to a trusted cogletd executable')
    args = parser.parse_args()
    try:
        return observe(args.cogletd, args.records)
    except (OSError, ValueError) as error:
        print(f'Observer failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
