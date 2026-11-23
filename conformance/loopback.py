#!/usr/bin/env python3
"""Harness fixture only, not a surface/runtime/hub conformance implementation."""
import argparse
import hashlib
import socket
import sys

from wire import MAX_FRAME, decode, encode


def serve(reader, writer):
    while header := reader.read(4):
        if len(header) != 4:
            raise ValueError('partial header')
        size = int.from_bytes(header, 'big')
        if not 0 < size <= MAX_FRAME:
            raise ValueError('invalid frame size')
        raw = reader.read(size)
        if len(raw) != size:
            raise ValueError('partial frame')
        try:
            value = decode(raw)
            if isinstance(value, dict) and value.get('loopback') == 'hash':
                value = {'state_hash': hashlib.sha256(encode(value['state'])).hexdigest()}
        except (ValueError, UnicodeError):
            value = {'error': {'code': 'invalid_argument', 'message': 'invalid CBOR', 'retryable': False}}
        raw = encode(value)
        writer.write(len(raw).to_bytes(4, 'big') + raw)
        writer.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--socket')
    args = parser.parse_args()
    if not args.socket:
        serve(sys.stdin.buffer, sys.stdout.buffer)
        return
    with socket.socket(socket.AF_UNIX) as server:
        server.bind(args.socket)
        server.listen()
        while True:
            connection, _ = server.accept()
            with connection, connection.makefile('rb') as reader, connection.makefile('wb') as writer:
                serve(reader, writer)


if __name__ == '__main__':
    main()
