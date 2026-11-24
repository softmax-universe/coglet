"""Duplex-audio simulator surface: Linux, no real audio.

Uses the public Python surface SDK. Native capture/playback is the DuplexEngine
clock, never a microphone or speaker. Scripted timelines come from
spec/fixtures/speech/timelines.json and the ADR-0010 handoff cases.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import tempfile
import time

from coglet_surface.surface import CATALOG, RESOURCES, Operation, Rejection, Surface, correlation_id
from coglet_surface.transport import serve

from .duplex import DuplexEngine

ROOT = Path(__file__).resolve().parents[2]
TIMELINES = json.loads((ROOT / 'spec/fixtures/speech/timelines.json').read_text())
CONFIG = json.loads((ROOT / 'spec/fixtures/surface/adapter-config.json').read_text())
COGLET = CONFIG['grant']['entries'][0]['coglet']
HELLO_ID = CONFIG['hello']['body']['correlation_id']
ITEM = '1' * 32
STREAM = '2' * 32
UTTERANCE = '1' * 32
TURN = '4' * 32
CALL = '3' * 32
PROVIDER_ITEM = '1' * 32
PROVIDER_UTTERANCE = '4' * 32

AUDIO_OPS = (
    'speech.start', 'speech.partial', 'speech.end', 'speech.position', 'speech.barge_in',
    'speech.config', 'speech.speak_chunk', 'speech.stop', 'speech.backchannel',
    'expression.set_expression', 'expression.state',
)
HANDOFF_OPS = AUDIO_OPS + (
    'speech.heard', 'speech.work_state', 'speech.work_intent', 'speech.work_done', 'speech.work_result',
)


class MarkerSurface(Surface):
    """Advertise empty capture-mode markers (not catalog operations)."""

    def __init__(self, *args, marker=None, **kwargs):
        super().__init__(*args, **kwargs)
        self.marker = marker

    def _capabilities(self):
        if not self.marker:
            return super()._capabilities()
        entry = dict(name=self.marker, events=[], reads=[], actions=[], exclusive=[], schemas={})
        self._emit('capabilities', dict(revision=self.revision, clock=self.now(),
                    entries=[entry], correlation_id=correlation_id()))


class DuplexDevice:
    def __init__(self, case, clock, surface_holder):
        self.case = case
        self.clock = clock
        self.surface_holder = surface_holder
        self.engine = DuplexEngine(barge='duck' if case == 'over-talk-duck' else 'stop',
                                   render_latency_ms=0)
        self.origin_ns = time.monotonic_ns()
        self.due = []
        self.started = False
        self.epoch = 1
        if case == 'heard-truncation':
            self.engine.spans = [
                {'end_ms': 1000, 'text': 'Hello'},
                {'end_ms': 3000, 'text': ' world'},
            ]

    def stop(self):
        self.engine.output = 'stopped' if self.engine.output == 'playing' else self.engine.output
        self.due.clear()

    def now_ms(self):
        return (time.monotonic_ns() - self.origin_ns) // 1_000_000

    def surface(self):
        return self.surface_holder[0]

    def emit_engine(self, events):
        surface = self.surface()
        for event in events:
            if event['name'] == 'action.outcome':
                payload = event['payload']
                if payload['status'] == 'cancelled':
                    surface.complete(COGLET, payload['id'], status='cancelled',
                                     error=payload.get('error') or Rejection('cancelled', 'No grant').error)
                elif payload['status'] == 'succeeded':
                    surface.complete(COGLET, payload['id'], result=payload.get('result') or {})
                continue
            surface.event(COGLET, event['name'], event['payload'], epoch=self.epoch)

    def schedule(self, at_ms, fn):
        self.due.append((at_ms, fn))

    def tick(self):
        now = self.now_ms()
        remain = []
        for at_ms, fn in self.due:
            if now >= at_ms:
                fn()
            else:
                remain.append((at_ms, fn))
        self.due = remain

    def prepare(self, name, args):
        if name in ('expression.set_expression', 'expression.state', 'speech.config',
                    'speech.heard', 'speech.work_state'):
            return
        if name == 'speech.speak_chunk':
            if self.engine.stream and self.engine.output in ('playing', 'ducked'):
                raise Rejection('conflict', 'stream already active')
            return
        if name == 'speech.stop':
            if args['stream'] != self.engine.stream and self.engine.stream is not None:
                raise Rejection('not_found', 'unknown stream')
            return
        if name == 'speech.work_result':
            if not any(w['call'] == args['call'] and w['epoch'] == args['epoch']
                       for w in self.engine.pending_work):
                raise Rejection('not_found', 'unknown work')
            return
        if name == 'speech.backchannel':
            raise Rejection('conflict', 'backchannel not used by duplex timelines')
        if name in ('speech.session_open', 'speech.session_close', 'speech.tool_result'):
            raise Rejection('unavailable', 'no provider session')
        if 'stream' in args or 'session' in args:
            return

    def execute(self, name, args):
        now = self.now_ms()
        if name == 'speech.config':
            config = dict(CONFIG['speech_config'])
            config['barge'] = self.engine.barge
            return config
        if name == 'expression.set_expression':
            return self.engine.set_expression(now, args['expression'], args['intensity'], args['duration_ms'])
        if name == 'expression.state':
            return self.engine.expression_state(now)
        if name == 'speech.heard':
            record = self.engine.heard(args['item'])
            if record is None:
                raise Rejection('not_found', 'unknown item')
            return record
        if name == 'speech.work_state':
            return self.engine.work_state()
        if name == 'speech.speak_chunk':
            events = self.engine.feed(now, 'play', stream=args['stream'], chunk=args['chunk'],
                                      item=ITEM, played_ms=0, position_ms=0)
            self.emit_engine(events)
            self.arm_after_play()
            return None
        if name == 'speech.stop':
            if self.engine.output in ('playing', 'ducked'):
                self.engine.output = 'stopped'
                self.engine.terminal = 'cancelled'
            return {}
        if name == 'speech.work_result':
            done = self.engine.work_result(args['turn'], args['call'], args['epoch'], args['value'])
            self.surface().event(COGLET, 'speech.work_done', done, epoch=self.epoch)
            return {}
        raise Rejection('unavailable')

    def arm_after_play(self):
        if self.case == 'barge-in-stop':
            self._arm_timeline('barge-in-stop')
        elif self.case == 'over-talk-duck':
            self._arm_timeline('over-talk-duck')
        elif self.case == 'heard-truncation':
            self.schedule(self.now_ms(), self._handoff_barge)

    def _timeline(self, name):
        return next(t for t in TIMELINES if t['name'] == name)

    def _arm_timeline(self, name):
        for step in self._timeline(name)['steps']:
            if step['input'] == 'play':
                continue
            payload = {k: v for k, v in step.items() if k not in ('input', 'at_ms')}
            kind = step['input']
            # Wire cases only wait at_ms=100 for the first capture; later media times
            # are scripted positions, not wall-clock delays.
            if kind in ('start', 'barge_in'):
                at_ms = min(step['at_ms'], 100)
            else:
                at_ms = min(step['at_ms'], 110)
            self.schedule(at_ms, lambda k=kind, p=payload, t=step['at_ms']: self.emit_engine(
                self.engine.feed(t, k, **p)))

    def _handoff_barge(self):
        events = []
        events.extend(self.engine.feed(1200, 'start', utterance=PROVIDER_UTTERANCE, position_ms=0))
        events.extend(self.engine.feed(1200, 'barge_in', utterance=PROVIDER_UTTERANCE,
                                       stream=self.engine.stream or STREAM, item=self.engine.item or PROVIDER_ITEM,
                                       played_ms=1200, position_ms=1200))
        self.emit_engine(events)

    def configured(self):
        self.started = True
        if self.case == 'late-partial-and-stale-revision':
            self._arm_timeline(self.case)
        elif self.case == 'work-cancelled':
            self.engine.feed(0, 'play', stream=STREAM, chunk=0, item=PROVIDER_ITEM, played_ms=0)
            self.engine.events.clear()
            self.emit_engine(self.engine.stage_work(0, TURN, CALL, 'game.inspect', {'request': 'Inspect'}))
            self._handoff_barge()
        elif self.case == 'duplicate-call':
            args = dict(turn=TURN, call=CALL, name='game.inspect', arguments={'request': 'Inspect'})
            self.emit_engine(self.engine.stage_work(0, **args))
            self.engine.stage_work(0, **args)

    def install(self, surface, names):
        for name in names:
            binding = CATALOG[name]
            resources = RESOURCES.get(name, ())
            if name == 'speech.heard':
                resources = ('audio.speaker',)
            surface.register(Operation(
                name, binding['entry'], binding['direction'],
                binding['input'].replace(':', '::'),
                binding['output'].replace(':', '::') if binding['output'] else None,
                resources,
                lambda args, context, n=name: self.prepare(n, args),
                lambda args, context, n=name: self.execute(n, args),
            ))


def case_name():
    return os.environ.get('COGLET_DUPLEX_CASE') or os.environ.get('COGLET_CONFORMANCE_CASE') or ''


def operations_for(case):
    if case.startswith('mode-'):
        return ()
    if case in ('heard-truncation', 'work-cancelled', 'duplicate-call'):
        return HANDOFF_OPS
    return AUDIO_OPS


def marker_for(case):
    if case == 'mode-hands-free':
        return 'speech.hands_free'
    if case == 'mode-push-to-talk':
        return 'speech.push_to_talk'
    return None


def grant_entries(names):
    resources = []
    for name in names:
        resources.extend(RESOURCES.get(name, ()))
        if name == 'speech.heard':
            resources.append('audio.speaker')
    return [dict(coglet=COGLET, operations=list(names),
                 resources=sorted(set(resources)), expires=5_000_000)]


def bind_session(case, ledger):
    names = operations_for(case)
    marker = marker_for(case)
    started = time.monotonic_ns()
    negotiated = [False]
    clock = lambda: CONFIG['clock'] + ((time.monotonic_ns() - started) // 1000 if negotiated[0] else 0)
    holder = [None]
    device = DuplexDevice(case, clock, holder)
    surface = MarkerSurface(CONFIG['surface'], {COGLET}, ledger, create=True, clock=clock,
                            stop=device.stop, marker=marker)
    holder[0] = surface
    if names:
        device.install(surface, names)

    def configured():
        if names:
            surface.grant(grant_entries(names))
        else:
            surface.grant([])
        negotiated[0] = True
        device.origin_ns = time.monotonic_ns()
        device.configured()

    return surface, device, configured


def unix_supervisor(directory):
    from contextlib import contextmanager
    import json as json_mod
    import socket
    import threading
    from coglet_surface.transport import UnixListener

    root = Path(directory)

    @contextmanager
    def run():
        errors = []
        control_path, path = root / 'control', root / 'wire'
        uid = os.getuid()
        with UnixListener(control_path, runtime_uid=uid) as control:
            with UnixListener(path, runtime_uid=uid) as wire:
                control.socket.settimeout(10)
                wire.socket.settimeout(10)

                def loop():
                    index = 0
                    try:
                        while True:
                            with control.accept() as peer:
                                raw = b''
                                while not raw.endswith(b'\n'):
                                    raw += peer.recv(4096)
                                setup = json_mod.loads(raw)
                                if setup.get('stop'):
                                    return
                                index += 1
                                case = setup.get('case') or ''
                                surface, device, configured = bind_session(
                                    case, root / f'ledger-{index}')
                                peer.sendall(b'ready\n')
                            with wire.accept() as connection:
                                try:
                                    serve(surface, connection.fileno(), connection.fileno(),
                                          hello_id=HELLO_ID, configured=configured, poll=device.tick)
                                finally:
                                    surface.close()
                    except BrokenPipeError:
                        pass
                    except BaseException as exc:
                        errors.append(exc)

                thread = threading.Thread(target=loop, daemon=True)
                thread.start()
                try:
                    yield path, control_path
                finally:
                    if thread.is_alive():
                        with socket.socket(socket.AF_UNIX) as peer:
                            peer.connect(str(control_path))
                            peer.sendall(b'{"stop":true}\n')
                        thread.join(timeout=15)
                    assert not thread.is_alive(), 'duplex supervisor failed to stop'
                    assert not errors, errors
    return run()


def main():
    surface, device, configured = bind_session(case_name(), Path(tempfile.mkdtemp(prefix='duplex-')) / 'ledger')
    try:
        serve(surface, sys.stdin.fileno(), sys.stdout.fileno(), hello_id=HELLO_ID,
              configured=configured, poll=device.tick)
    finally:
        surface.close()


if __name__ == '__main__':
    main()
