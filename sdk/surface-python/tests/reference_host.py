"""Test-only supervisor; provision native state before harness timing begins."""
from contextlib import contextmanager
import json
from pathlib import Path
import socket
import threading
import time

from coglet_surface.reference import ReferenceDevice
from coglet_surface.surface import Surface
from coglet_surface.transport import UnixListener, serve


@contextmanager
def reference_host(directory, config):
    directory = Path(directory)
    errors = []
    control_path, path = directory / 'control', directory / 'wire'
    with UnixListener(control_path, runtime_uid=__import__('os').getuid()) as control:
        with UnixListener(path, runtime_uid=__import__('os').getuid()) as wire:
            control.socket.settimeout(10)
            wire.socket.settimeout(10)
            def run():
                index = 0
                try:
                    while True:
                        with control.accept() as peer:
                            raw = b''
                            while not raw.endswith(b'\n'):
                                raw += peer.recv(4096)
                            setup = json.loads(raw)
                            if setup.get('stop'):
                                return
                            index += 1
                            active = [False]
                            origin = [0]
                            clock = lambda: config['clock'] + (
                                (time.monotonic_ns() - origin[0]) // 1000 if active[0] else 0)
                            device = ReferenceDevice(config, clock)
                            surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                                directory / f'ledger-{index}', create=True, clock=clock, stop=device.stop)
                            device.install(surface, sorted({op for g in config['grant']['entries'] for op in g['operations']}))
                            surface._set('epoch', config['startup_epoch_by_case'].get(setup['case'], 1) - 1)
                            surface._set('peer_clock', 0)
                            surface.now()
                            peer.sendall(b'ready\n')
                        with wire.accept() as connection:
                            def configured():
                                surface.grant(config['grant']['entries'])
                                origin[0] = time.monotonic_ns()
                                active[0] = True
                            killed = [False]
                            def poll():
                                device.tick()
                                if (active[0] and setup['kill_ms'] and not killed[0]
                                        and clock() >= int(setup['kill_ms']) * 1000):
                                    killed[0] = True
                                    surface.kill('local operator off-switch')
                            try:
                                serve(surface, connection.fileno(), connection.fileno(),
                                    hello_id=config['hello']['body']['correlation_id'],
                                    configured=configured, poll=poll)
                            finally:
                                surface.close()
                except BaseException as exc:
                    errors.append(exc)
            thread = threading.Thread(target=run, daemon=True)
            thread.start()
            try:
                yield path, control_path
            finally:
                if thread.is_alive():
                    with socket.socket(socket.AF_UNIX) as peer:
                        peer.connect(str(control_path))
                        peer.sendall(b'{"stop":true}\n')
                    thread.join(timeout=15)
                assert not thread.is_alive(), 'reference supervisor failed to stop'
                assert not errors, errors
