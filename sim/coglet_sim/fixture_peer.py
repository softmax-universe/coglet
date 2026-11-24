"""Test-only ADR-0024 scheduled duplex peer; never a production transport.

Wire bytes use the public SDK codec/framing. A private scheduler controls both
protocol and accelerated media clocks without replacing operations or the ledger.
"""
import json
import os
from pathlib import Path
import select
import socket
import sys
import tempfile

from coglet_surface.codec import decode
from coglet_surface.surface import WIRE
from coglet_surface.transport import Frames, frame
from .host import CONFIG, HELLO_ID, bind_session, case_name


class ScheduledSession:
    def __init__(self, directory, write):
        self.time = 0
        self.bytes = 0
        self.write = write
        self.surface, self.device, self.configured = bind_session(
            case_name(), Path(directory) / 'ledger', fixture_time=lambda: self.time)
        self.ready = False
        self.emit(self.surface.open(HELLO_ID))

    def emit(self, raw):
        encoded = frame(raw)
        self.write(encoded)
        self.bytes += len(encoded)

    def flush(self):
        for raw in self.surface.drain():
            self.emit(raw)

    def ack(self):
        return json.dumps({'time': self.time, 'bytes': self.bytes}).encode() + b'\n'

    def dispatch(self, raw):
        self.surface.receive(raw)
        if self.surface.state == 'ready' and not self.ready:
            self.ready = True
            self.configured()
        self.device.tick()
        self.surface.tick()
        self.flush()

    def next_time(self, limit):
        # Include SDK timers as well as media callbacks: no expiry, grant or
        # health boundary may be skipped by a long explicit clock advance.
        times = [limit, self.surface.last_received + 3_000_000 - CONFIG['clock']]
        times += [entry[0] * 1000 for entry in self.device.due]
        times += [g['expires'] - CONFIG['clock'] for g in self.surface.grants]
        times += [decode(WIRE, raw)['body']['action']['deadline'] - CONFIG['clock']
                  for _, _, raw in self.surface._pending()]
        if self.surface.state == 'ready':
            times.append(self.surface.last_health + 1_000_000 - CONFIG['clock'])
        return min(max(self.time, value) for value in times)

    def advance(self, operation, target):
        if operation not in ('advance', 'next') or type(target) is not int or not self.time <= target < 2**63:
            raise ValueError('invalid scheduled clock control')
        # An explicit advance must run every intermediate timer chronologically.
        while True:
            self.time = self.next_time(target)
            self.device.tick()
            self.surface.tick()
            self.flush()
            if operation == 'next' or self.time == target:
                return

    def close(self):
        try:
            self.surface.disconnect()
        finally:
            self.surface.close()


def main():
    # Opt-in is mandatory. Ordinary host.main and Unix supervisor never open
    # this channel, and inherited harness state is removed for ordinary peers.
    path = os.environ['COGLET_FIXTURE_CLOCK_SOCKET']
    with socket.socket(socket.AF_UNIX) as clock, tempfile.TemporaryDirectory(prefix='duplex-clock-') as directory:
        clock.connect(path)
        def write(raw):
            sys.stdout.buffer.write(raw)
            sys.stdout.buffer.flush()
        session = ScheduledSession(directory, write)
        parser, control = Frames(), bytearray()
        try:
            while True:
                readable, _, _ = select.select([clock, sys.stdin.fileno()], [], [])
                if clock in readable:
                    chunk = clock.recv(256)
                    if not chunk:
                        return
                    control.extend(chunk)
                    if len(control) > 256:
                        raise ValueError('oversized scheduled clock control')
                    if b'\n' in control:
                        raw, _, remain = control.partition(b'\n')
                        control = bytearray(remain)
                        command = json.loads(raw)
                        if not isinstance(command, dict) or len(command) != 1:
                            raise ValueError('invalid scheduled clock control')
                        operation, target = next(iter(command.items()))
                        session.advance(operation, target)
                        clock.sendall(session.ack())
                if sys.stdin.fileno() in readable:
                    chunk = os.read(sys.stdin.fileno(), 65536)
                    if not chunk:
                        parser.eof()
                        return
                    for raw in parser.feed(chunk):
                        session.dispatch(raw)
                        clock.sendall(session.ack())
        finally:
            session.close()


if __name__ == '__main__':
    main()
