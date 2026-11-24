"""Duplex-audio simulator surface: Linux, no real audio.

Uses the public Python surface SDK. Native capture/playback is the DuplexEngine
clock, never a microphone or speaker. Scripted user speech comes from
spec/fixtures/speech/timelines.json and the ADR-0010 handoff cases; playback
items are always the admitted speak action IDs from the RequestContext, never
fixture constants.

Timing: render starts RENDER_LATENCY_MS (wall) after admission, then virtual
media advances SPEEDUP times faster than wall time, so a 3 s utterance plays in
300 ms of CI time. Media positions reported on the wire are virtual media ms.
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
STREAM = '2' * 32
TURN = '4' * 32
CALL = '3' * 32
PROVIDER_ITEM = '1' * 32
PROVIDER_UTTERANCE = '4' * 32
HISTORY_UTTERANCE = '6' * 32

SPEEDUP = 10
RENDER_LATENCY_MS = 20
BYTE_MS = 50  # unaligned voice: media duration per UTF-8 byte, no alignment

AUDIO_OPS = (
    'speech.start', 'speech.partial', 'speech.end', 'speech.position', 'speech.barge_in',
    'speech.config', 'speech.speak_chunk', 'speech.stop', 'speech.backchannel',
    'expression.set_expression', 'expression.state',
)
HANDOFF_OPS = AUDIO_OPS + (
    'speech.heard', 'speech.work_state', 'speech.work_intent', 'speech.work_done', 'speech.work_result',
)

# ADR-0010 heard-truncation fixture alignment: 'Hello' ends at 1000 ms, ' world' at 3000 ms.
HELLO_WORLD = ([{'end_ms': 1000, 'text': 'Hello'}, {'end_ms': 3000, 'text': ' world'}], 3000)
GOOD_MORNING = ([{'end_ms': 600, 'text': 'Good'}, {'end_ms': 1400, 'text': ' morning.'}], 1400)


def _timeline(name):
    return next(t for t in TIMELINES if t['name'] == name)


def _capture(name):
    steps = []
    for step in _timeline(name)['steps']:
        if step['input'] in ('play', 'finish'):
            continue
        steps.append(dict(step))
    return steps


def _barge_at(ms, utterance):
    return [
        {'at_ms': ms, 'input': 'start', 'utterance': utterance, 'position_ms': 0},
        {'at_ms': ms, 'input': 'barge_in', 'utterance': utterance, 'played_ms': ms},
    ]


def script_for(case):
    """Voice alignment table and scripted user speech for one scenario.

    capture.chunk names which admitted chunk of the first stream the user speech
    is timed against; barge_in steps target whatever item is actually playing.
    """
    if case == 'barge-in-stop':
        return {'voice': {}, 'capture': {'chunk': 0, 'steps': _capture(case)}}
    if case == 'over-talk-duck':
        finish = next(s for s in _timeline(case)['steps'] if s['input'] == 'finish')
        return {'voice': {'Scripted output': ([], finish['played_ms'])},
                'capture': {'chunk': 0, 'steps': _capture(case)}}
    if case == 'heard-truncation':
        return {'voice': {'Hello world': HELLO_WORLD},
                'capture': {'chunk': 0, 'steps': _barge_at(1200, PROVIDER_UTTERANCE)}}
    if case == 'heard-history':
        # Finished aligned item, finished unaligned item, interrupted item, queued item.
        return {'voice': {'Hello world': HELLO_WORLD, 'Good morning.': GOOD_MORNING},
                'capture': {'chunk': 2, 'steps': _barge_at(1200, HISTORY_UTTERANCE)}}
    return {'voice': {}, 'capture': None}


def barge_for(case):
    return 'duck' if case == 'over-talk-duck' else 'stop'


class MarkerSurface(Surface):
    """Advertise empty capture-mode markers (not catalog operations).

    Exclusive resources are declared once, on the entry that owns the device
    (speech.control / expression.control). Optional entries such as
    speech.history and speech.work still require those leases in grants, but
    re-advertising them as exclusive there makes the runtime refuse the whole
    capability snapshot (one owner entry per resource).
    """

    OWNERS = ('speech.control', 'speech.session', 'expression.control')

    def __init__(self, *args, marker=None, revoked=None, **kwargs):
        super().__init__(*args, **kwargs)
        self.marker = marker
        self.revoked = revoked

    # Every grant snapshot (and kill) moves authority to a new epoch: the SDK
    # cancels all earlier-epoch work. Heard records of revoked owners go too.
    def grant(self, entries):
        super().grant(entries)
        if self.revoked:
            self.revoked()

    def kill(self, reason):
        super().kill(reason)
        if self.revoked:
            self.revoked()

    def _emit(self, kind, body):
        if kind == 'capabilities':
            body = dict(body, entries=[
                dict(entry, exclusive=entry['exclusive'] if entry['name'] in self.OWNERS else [])
                for entry in body['entries']])
        super()._emit(kind, body)

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
        self.script = script_for(case)
        self.engine = DuplexEngine(barge=barge_for(case), render_latency_ms=0)
        self.origin_ns = time.monotonic_ns()
        self.due = []
        self.started = False
        self.generation = 0
        self.capture_stream = None
        self.captured = False
        self.native_epoch = None  # grant epoch owning provider-native items

    # Native off-switch and callback fencing --------------------------------

    def stop(self):
        """Surface reflex: silence all output and fence every queued callback."""
        self.engine.halt()
        self.generation += 1
        self.due.clear()

    def heard_owner(self, item):
        """(coglet, epoch) whose authority covers this item's heard record."""
        record = self.engine.items[item]
        if record['owner'] is None:
            return COGLET, self.native_epoch
        return tuple(record['owner'])

    def revoke_heard(self):
        """Remove heard records whose owner authority was revoked (speech.md:
        retain for the action-retention interval, remove on owner revocation).

        A regrant to the same coglet is a new epoch, not a continuation: items
        admitted under an earlier epoch are no longer readable. Tombstones stay
        in the playback table so stopped streams and fenced callbacks hold."""
        surface = self.surface()
        epoch = surface._get('epoch')
        holders = {g['coglet'] for g in surface.grants if 'speech.heard' in g['operations']}
        self.engine.forget_heard([
            item for item, record in self.engine.items.items()
            if not record.get('heard_removed')
            and (self.heard_owner(item)[1] != epoch or self.heard_owner(item)[0] not in holders)])

    def now_ms(self):
        return (time.monotonic_ns() - self.origin_ns) // 1_000_000

    def surface(self):
        return self.surface_holder[0]

    def schedule(self, at_ms, fn):
        generation = self.generation
        self.due.append((at_ms, generation, fn))

    def tick(self):
        now = self.now_ms()
        due, remain = [], []
        for entry in self.due:
            (due if now >= entry[0] else remain).append(entry)
        self.due = remain
        for _, generation, fn in sorted(due, key=lambda e: e[0]):
            if generation == self.generation:
                fn()

    def voice(self, text):
        spans, duration = self.script['voice'].get(text, ([], None))
        if duration is None:
            duration = BYTE_MS * len(text.encode())
        return spans, duration

    def owner(self, item):
        record = self.engine.items.get(item)
        if record is None or record['owner'] is None:
            return COGLET, self.surface()._get('epoch')
        return tuple(record['owner'])

    def emit_engine(self, events):
        surface = self.surface()
        for event in events:
            payload = event['payload']
            if event['name'] == 'action.outcome':
                record = self.engine.items.get(payload['id'])
                if record is None or record['owner'] is None:
                    continue  # provider-native item: no admitted action to resolve
                coglet = record['owner'][0]
                if payload['status'] == 'succeeded':
                    surface.complete(coglet, payload['id'], result=payload['result'])
                else:
                    surface.complete(coglet, payload['id'], status=payload['status'],
                                     error=payload['error'])
                continue
            item = payload.get('item')
            coglet, epoch = self.owner(item) if item else (COGLET, surface._get('epoch'))
            try:
                surface.event(coglet, event['name'], payload, epoch=epoch)
            except Rejection:
                # Late callback after revocation/regrant: fenced, never redirected.
                self.stop()

    # Playback scheduling -----------------------------------------------------

    def _render_next(self, stream):
        item = self.engine.next_queued(stream)
        if item is None:
            return
        self.emit_engine(self.engine.render(self.now_ms() * SPEEDUP, item))
        record = self.engine.items[item]
        started = self.now_ms()
        self.schedule(started + record['duration_ms'] // SPEEDUP,
                      lambda: self._finish(item))
        capture = self.script['capture']
        if (capture and not self.captured and stream == self.capture_stream
                and record['chunk'] == capture['chunk']):
            self.captured = True
            self._arm_capture(started, capture['steps'], stream)

    def _finish(self, item):
        record = self.engine.items[item]
        if record['state'] not in ('playing', 'ducked'):
            return
        self.emit_engine(self.engine.complete_item(self.now_ms() * SPEEDUP, item))
        self._render_next(record['stream'])

    def _arm_capture(self, started, steps, stream=None):
        for step in steps:
            payload = {k: v for k, v in step.items() if k not in ('input', 'at_ms')}
            self.schedule(started + step['at_ms'] // SPEEDUP,
                          lambda k=step['input'], p=payload, s=stream: self._capture(k, p, s))

    def _capture(self, kind, payload, stream):
        if kind == 'barge_in':
            item = self.engine.active_item()
            if item is None or (stream is not None and self.engine.items[item]['stream'] != stream):
                return  # nothing audible any more: user speech emits start only
            payload = dict(payload, item=item, stream=self.engine.items[item]['stream'])
            payload.pop('position_ms', None)
            if payload['played_ms'] > self.engine.items[item]['duration_ms']:
                return
        self.emit_engine(self.engine.feed(self.now_ms() * SPEEDUP, kind, **payload))

    def _handoff_barge(self):
        events = []
        events.extend(self.engine.feed(1200, 'start', utterance=PROVIDER_UTTERANCE, position_ms=0))
        events.extend(self.engine.feed(1200, 'barge_in', utterance=PROVIDER_UTTERANCE,
                                       stream=self.engine.stream, item=self.engine.item,
                                       played_ms=1200, position_ms=1200))
        self.emit_engine(events)

    # Operations ------------------------------------------------------------------

    def prepare(self, name, args, context):
        if name in ('expression.set_expression', 'expression.state', 'speech.config',
                    'speech.heard', 'speech.work_state'):
            return
        if name == 'speech.speak_chunk':
            stream = self.engine.streams.get(args['stream'])
            if stream and stream['owner'] and stream['owner'][0] != context.coglet:
                raise Rejection('conflict', 'stream owned by another coglet')
            if stream and (stream['stopped'] or stream['final']):
                raise Rejection('conflict', 'stream closed')
            if (len(stream['items']) if stream else 0) != args['chunk']:
                raise Rejection('conflict', 'noncontiguous chunk')
            if stream and len(stream['items']) >= 64:
                raise Rejection('conflict', 'chunk limit')
            if any(s != args['stream'] and self.engine.stream_open(s) for s in self.engine.streams):
                raise Rejection('conflict', 'another stream is open')
            return
        if name == 'speech.stop':
            stream = self.engine.streams.get(args['stream'])
            if stream is None or (stream['owner'] and stream['owner'][0] != context.coglet):
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

    def execute(self, name, args, context):
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
            if record is None or self.heard_owner(args['item']) != (context.coglet, context.epoch):
                raise Rejection('not_found', 'unknown item')
            return record
        if name == 'speech.work_state':
            return self.engine.work_state()
        if name == 'speech.speak_chunk':
            spans, duration = self.voice(args['text'])
            stream = args['stream']
            idle = self.engine.next_queued(stream) is None and not any(
                self.engine.items[i]['state'] in ('playing', 'ducked')
                for i in self.engine.streams.get(stream, {'items': []})['items'])
            # Native item identity is exactly the admitted wire action ID.
            self.engine.enqueue(stream, args['chunk'], context.action_id, spans, duration,
                                args['final'], owner=[context.coglet, context.epoch])
            if self.capture_stream is None:
                self.capture_stream = stream
            if idle:
                self.schedule(now + RENDER_LATENCY_MS, lambda: self._render_next(stream))
            return None  # completes only after the chunk is actually rendered
        if name == 'speech.stop':
            self.emit_engine(self.engine.stop_stream(now * SPEEDUP, args['stream']))
            return {}
        if name == 'speech.work_result':
            done = self.engine.work_result(args['turn'], args['call'], args['epoch'], args['value'])
            self.surface().event(COGLET, 'speech.work_done', done, epoch=self.surface()._get('epoch'))
            return {}
        raise Rejection('unavailable')

    def configured(self):
        self.started = True
        self.native_epoch = self.surface()._get('epoch')
        if self.case == 'late-partial-and-stale-revision':
            self._arm_capture(self.now_ms(), _capture(self.case))
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
                lambda args, context, n=name: self.prepare(n, args, context),
                lambda args, context, n=name: self.execute(n, args, context),
            ))


def case_name():
    return os.environ.get('COGLET_DUPLEX_CASE') or os.environ.get('COGLET_CONFORMANCE_CASE') or ''


def operations_for(case):
    if case.startswith('mode-'):
        return ()
    if case in ('heard-truncation', 'heard-history', 'work-cancelled', 'duplicate-call'):
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
                            stop=device.stop, marker=marker, revoked=device.revoke_heard)
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
