"""Test-only reference surface using the public SDK, never a response script.

Provisioning comes from the spec's adapter-config.json. This is a no-hardware
semantic expression renderer and an empty speech device, not speech conformance.
"""
import json
import os
from pathlib import Path
import select
import socket
import sys
import tempfile
from contextlib import nullcontext
import time
from typing import Any

from .surface import CATALOG, RESOURCES, Operation, Rejection, Surface
from .catalog import TrustedExtensionCatalog, profile as check_profile
from .transport import Frames, frame, serve

PROFILE_ENV = 'COGLET_CONFORMANCE_CONTRACT_PROFILE'
CATALOG_ENV = 'COGLET_FIXTURE_CATALOG'
CATALOG_PIN_ENV = 'COGLET_FIXTURE_CATALOG_SHA256'


def provision(environ=None) -> dict[str, Any]:
    """Trusted local provisioning from the supervisor environment (ADR-0030 1a).

    Returns Surface keyword arguments. An absent selector is surface-0.1; a present
    but empty/unknown one fails closed. The catalog path needs its separate pin.
    Nothing here comes from a wire frame, a manifest or the peer.
    """
    environ = os.environ if environ is None else environ
    selected = check_profile(environ[PROFILE_ENV]) if PROFILE_ENV in environ else 'surface-0.1'
    path, pin = environ.get(CATALOG_ENV), environ.get(CATALOG_PIN_ENV)
    if (path is None) != (pin is None):
        raise ValueError('catalog path and sha256 pin must be supplied together')
    trusted = None
    if path is not None:
        trusted = TrustedExtensionCatalog(Path(path).read_bytes(), pin, contract_profile=selected)
    return dict(contract_profile=selected, trusted_catalog=trusted)


class ReferenceDevice:
    def __init__(self, config, clock):
        self.config, self.clock = config, clock
        self.stop()

    def stop(self):
        self.expression = 'neutral'
        self.intensity = 0
        self.until = 0

    def prepare(self, name, args):
        if name in ('expression.set_expression', 'expression.state', 'speech.config'):
            return
        if name not in CATALOG:
            return  # pinned-catalog app operation; schema validation already ran
        if 'stream' in args or 'session' in args:
            raise Rejection('not_found', 'unknown stream or session')
        raise Rejection('unavailable', 'reference has no speech provider')

    def tick(self):
        if self.clock() >= self.until:
            self.stop()

    def execute(self, name, args):
        now = self.clock()
        if name == 'speech.config':
            return self.config['speech_config']
        if name == 'expression.set_expression':
            duration = args['duration_ms']
            self.expression = args['expression'] if duration else 'neutral'
            self.intensity = args['intensity'] if duration else 0
            self.until = now + duration * 1000
            return {}
        if name == 'expression.state':
            if now >= self.until:
                self.stop()
            return dict(expression=self.expression, intensity=self.intensity,
                        remaining_ms=max(0, (self.until - now + 999) // 1000))
        if name not in CATALOG:
            # Test-only semantic stand-in for an app: one fixed title, no real app.
            return {'title': 'hello'} if name.endswith('.state') else {}
        raise Rejection('unavailable')

    def install(self, surface, names):
        for name in names:
            if name in CATALOG:
                binding = CATALOG[name]
                resources = RESOURCES.get(name, ())
                surface.register(Operation(name, binding['entry'], binding['direction'],
                    binding['input'].replace(':', '::'),
                    binding['output'].replace(':', '::') if binding['output'] else None,
                    resources, lambda args, context, n=name: self.prepare(n, args),
                    lambda args, context, n=name: self.execute(n, args)))
                continue
            app = surface.catalog[name]  # only operations of the pinned catalog exist here
            surface.register(Operation(name, app['entry'], app['direction'], app['input'],
                app.get('output'), tuple(app['resources']), lambda args, context, n=name: self.prepare(n, args),
                lambda args, context, n=name: self.execute(n, args)))


def main():
    if '--conformance-clock' in sys.argv:
        return conformance_clock()
    config = json.loads(Path(os.environ['COGLET_FIXTURE_CONFIG']).read_text())
    ledger = Path(os.environ['COGLET_FIXTURE_LEDGER'])
    if '--cleanup' in sys.argv:
        for suffix in ('', '-wal', '-shm'):
            Path(str(ledger) + suffix).unlink(missing_ok=True)
        return
    if '--provision' in sys.argv:
        surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                          ledger, create=True, clock=lambda: config['clock'], stop=lambda: None, **provision())
        epoch = config['startup_epoch_by_case'].get(os.environ.get('COGLET_CONFORMANCE_CASE'), 1)
        surface._set('epoch', epoch - 1)
        surface.close()
        return
    started = time.monotonic_ns()
    negotiated = [False]
    kill_started = [None]
    clock = lambda: config['clock'] + ((time.monotonic_ns() - started) // 1000 if negotiated[0] else 0)
    device = ReferenceDevice(config, clock)
    with nullcontext():
        surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                          ledger, clock=clock, stop=device.stop, **provision())
        names = {op for g in config['grant']['entries'] for op in g['operations']}
        device.install(surface, sorted(names))

        def configured():
            surface.grant(effective_grants(surface, config))
            negotiated[0] = True
            kill_started[0] = time.monotonic_ns()

        killed = [False]
        def poll():
            device.tick()
            kill_ms = os.environ.get('COGLET_FIXTURE_KILL_MS')
            if (kill_ms and kill_started[0] is not None and not killed[0]
                    and (time.monotonic_ns() - kill_started[0]) // 1_000_000 >= int(kill_ms)):
                killed[0] = True
                surface.kill('local operator off-switch')
        try:
            serve(surface, sys.stdin.fileno(), sys.stdout.fileno(),
                  hello_id=config['hello']['body']['correlation_id'], configured=configured, poll=poll)
        finally:
            surface.close()


def effective_grants(surface, config):
    """Grant only operations the negotiated wire version can carry (0.1 downgrade keeps built-ins)."""
    entries = []
    for entry in config['grant']['entries']:
        ops = [op for op in entry['operations'] if surface.version == '0.2' or op in CATALOG]
        if len(ops) == len(entry['operations']):
            entries.append(entry)  # frozen 0.1 path: byte-identical grants
            continue
        gone = {r for op in entry['operations'] if op not in ops for r in surface.catalog[op]['resources']}
        keep = {r for op in ops for r in RESOURCES.get(op, ())}
        entries.append(dict(entry, operations=ops, resources=[r for r in entry['resources'] if r not in gone or r in keep]))
    return entries


def conformance_clock():
    """Test-only ADR-0021 stdio adapter with the supervisor-provisioned contract profile."""
    config = json.loads(Path(os.environ['COGLET_FIXTURE_CONFIG']).read_text())
    logical = [0]
    clock = lambda: config['clock'] + logical[0]
    with tempfile.TemporaryDirectory(prefix='coglet-python-') as directory:
        device = ReferenceDevice(config, clock)
        surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                          Path(directory) / 'ledger', create=True, clock=clock, stop=device.stop, **provision())
        with socket.socket(socket.AF_UNIX) as control:
            try:
                device.install(surface, sorted({op for g in config['grant']['entries'] for op in g['operations']}))
                surface._set('epoch', config['startup_epoch_by_case'].get(os.environ.get('COGLET_CONFORMANCE_CASE'), 1) - 1)
                control.settimeout(3)
                control.connect(os.environ['COGLET_FIXTURE_CLOCK_SOCKET'])
                parser, advances, configured = Frames(), bytearray(), False

                def flush():
                    for raw in surface.drain():
                        sys.stdout.buffer.write(frame(raw))
                    sys.stdout.buffer.flush()

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
                            device.tick()
                            surface.tick()
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
                                    surface.grant(effective_grants(surface, config))
                                    configured = True
                                device.tick()
                                surface.tick()
                                flush()
                                control.sendall(logical[0].to_bytes(8, 'big'))
            finally:
                surface.close()


if __name__ == '__main__':
    main()
