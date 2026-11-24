"""Test-only ADR-0021 binding; real SDK/SQLite, never production clock control."""
import json
import os
from pathlib import Path
import select
import socket
import sys
import tempfile

# [DEBUG-python-startup] Attribute pre-main import and durable startup stalls.
import faulthandler
faulthandler.dump_traceback_later(2, repeat=True)

from coglet_surface.reference import ReferenceDevice
from coglet_surface.surface import Surface
from coglet_surface.transport import Frames, frame

ROOT = Path(__file__).resolve().parents[3]


def main():
    config = json.loads((ROOT / 'spec/fixtures/surface/adapter-config.json').read_text())
    logical = [0]
    clock = lambda: config['clock'] + logical[0]
    with tempfile.TemporaryDirectory(prefix='coglet-python-') as directory:
        device = ReferenceDevice(config, clock)
        surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                          Path(directory) / 'ledger', create=True, clock=clock, stop=device.stop)
        with socket.socket(socket.AF_UNIX) as control:
            try:
                device.install(surface, sorted({op for g in config['grant']['entries'] for op in g['operations']}))
                surface._set('epoch', config['startup_epoch_by_case'].get(os.environ['COGLET_CONFORMANCE_CASE'], 1) - 1)
                control.settimeout(3)
                control.connect(os.environ['COGLET_FIXTURE_CLOCK_SOCKET'])
                parser = Frames()
                advances = bytearray()
                configured = False
                killed = False

                def flush():
                    for raw in surface.drain():
                        sys.stdout.buffer.write(frame(raw))
                    sys.stdout.buffer.flush()

                def tick():
                    nonlocal killed
                    device.tick()
                    kill_ms = os.environ.get('COGLET_FIXTURE_KILL_MS')
                    if configured and kill_ms and not killed and logical[0] >= int(kill_ms) * 1000:
                        killed = True
                        surface.kill('local operator off-switch')
                    surface.tick()

                sys.stdout.buffer.write(frame(surface.open(config['hello']['body']['correlation_id'])))
                sys.stdout.buffer.flush()
                print('COGLET_PYTHON_READY', file=sys.stderr, flush=True)
                while True:
                    readable, _, _ = select.select([sys.stdin.fileno(), control], [], [], 3)
                    if not readable:
                        raise TimeoutError('fixture input watchdog')
                    for source in readable:
                        if source == control:
                            data = control.recv(4096)
                            if not data:
                                if advances:
                                    raise ValueError('partial fixture clock')
                                return
                            advances.extend(data)
                            if len(advances) > 8:
                                raise ValueError('unacknowledged fixture advances')
                            if len(advances) < 8:
                                continue
                            now = int.from_bytes(advances, 'big')
                            advances.clear()
                            if now < logical[0]:
                                raise ValueError('retrograde fixture clock')
                            logical[0] = now
                            tick()
                            flush()
                            control.sendall(now.to_bytes(8, 'big'))
                        else:
                            data = os.read(sys.stdin.fileno(), 65536)
                            if not data:
                                parser.eof()
                                return
                            for raw in parser.feed(data):
                                surface.receive(raw)
                                if surface.state == 'ready' and not configured:
                                    surface.grant(config['grant']['entries'])
                                    configured = True
                                tick()
                                flush()
                                # The harness sends one complete frame per
                                # barrier, then waits before advancing time.
                                control.sendall(logical[0].to_bytes(8, 'big'))
            finally:
                surface.close()


if __name__ == '__main__':
    main()
