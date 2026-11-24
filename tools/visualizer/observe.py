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
        child = None
        cancelled = False
        previous = {}
        def stop(signum, frame):
            # Nonraising handlers cover Popen's return/reference handoff. A
            # pending cancellation is applied immediately after child assignment.
            nonlocal cancelled
            cancelled = True
            if child is not None:
                child.terminate()
        try:
            for signum in (signal.SIGTERM, signal.SIGINT):
                previous[signum] = signal.signal(signum, stop)
            if cancelled:
                return 130
            child = subprocess.Popen([binary, '--mock-stdio'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            if cancelled:
                return 130
            assert child.stdin is not None and child.stdout is not None
            sink, source = child.stdin, child.stdout
            def forward():
                try:
                    while chunk := os.read(sys.stdin.fileno(), 65536):
                        sink.write(chunk)
                        sink.flush()
                except (BrokenPipeError, OSError):
                    pass
                finally:
                    sink.close()
            threading.Thread(target=forward, daemon=True).start()
            written = 0
            def exact(size):
                data = source.read(size)
                if len(data) != size:
                    raise ValueError('Truncated runtime response')
                return data
            while header := source.read(4):
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
            if child is not None:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill(); child.wait()
                if child.stdout is not None:
                    child.stdout.close()
                # The CLI exits after observe; its daemon stdin forwarder cannot
                # retain a live child. Do not block on the parent's stdin here.
            for signum, handler in previous.items():
                signal.signal(signum, handler)


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
