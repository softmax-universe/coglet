"""Test-only reference surface using the public SDK, never a response script.

Provisioning comes from the spec's adapter-config.json. This is a no-hardware
semantic expression renderer and an empty speech device, not speech conformance.
"""
import json
import os
from pathlib import Path
import sys
import tempfile
from contextlib import nullcontext
import time

from .surface import CATALOG, RESOURCES, Operation, Rejection, Surface
from .transport import serve


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
        raise Rejection('unavailable')

    def install(self, surface, names):
        for name in names:
            binding = CATALOG[name]
            resources = RESOURCES.get(name, ())
            surface.register(Operation(name, binding['entry'], binding['direction'],
                binding['input'].replace(':', '::'),
                binding['output'].replace(':', '::') if binding['output'] else None,
                resources, lambda args, context, n=name: self.prepare(n, args),
                lambda args, context, n=name: self.execute(n, args)))


def main():
    config = json.loads(Path(os.environ['COGLET_FIXTURE_CONFIG']).read_text())
    ledger = Path(os.environ['COGLET_FIXTURE_LEDGER'])
    if '--cleanup' in sys.argv:
        for suffix in ('', '-wal', '-shm'):
            Path(str(ledger) + suffix).unlink(missing_ok=True)
        return
    if '--provision' in sys.argv:
        surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                          ledger, create=True, clock=lambda: config['clock'], stop=lambda: None)
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
                          ledger, clock=clock, stop=device.stop)
        names = {op for g in config['grant']['entries'] for op in g['operations']}
        device.install(surface, sorted(names))

        def configured():
            surface.grant(config['grant']['entries'])
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


if __name__ == '__main__':
    main()
