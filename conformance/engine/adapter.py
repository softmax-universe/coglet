#!/usr/bin/env python3
"""Test-only CBOR pipe bridge for ANY engine's JSON-line adapter, not an oracle."""
import json
import pathlib
import struct
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from wire import decode, encode, MAX_FRAME


def exact(stream, size):
    data = stream.read(size)
    if len(data) != size:
        raise EOFError("truncated adapter frame")
    return data


def main():
    child = subprocess.Popen(sys.argv[1:], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    assert child.stdin is not None and child.stdout is not None
    try:
        while header := sys.stdin.buffer.read(4):
            if len(header) != 4:
                raise EOFError("truncated header")
            size = struct.unpack(">I", header)[0]
            if not 0 < size <= MAX_FRAME:
                raise ValueError("invalid frame length")
            request = decode(exact(sys.stdin.buffer, size))
            child.stdin.write(json.dumps(request, separators=(",", ":")).encode() + b"\n")
            child.stdin.flush()
            line = child.stdout.readline(MAX_FRAME + 1)
            if not line or len(line) > MAX_FRAME or not line.endswith(b"\n"):
                raise ValueError("invalid JSON-line engine response")
            raw = encode(json.loads(line))
            sys.stdout.buffer.write(struct.pack(">I", len(raw)) + raw)
            sys.stdout.buffer.flush()
    finally:
        child.stdin.close()
        child.terminate()
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()


if __name__ == "__main__":
    main()
