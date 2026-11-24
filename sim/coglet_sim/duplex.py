"""Deterministic duplex-audio engine: no waveform, no provider, no real clock.

Playback, barge-in, capture normalization and heard truncation follow
spec/capabilities/speech.md and the scripted timelines in
spec/fixtures/speech/timelines.json. Render latency is a small virtual delay
before the first playing position; media positions are scripted, never inferred
from wall time or text-duration ratios.
"""
from __future__ import annotations

from copy import deepcopy


def heard_prefix(spans, played_ms, limit=16384):
    """Conservative prefix of aligned spans whose end is at or before played_ms."""
    text = ''
    for span in spans:
        if span['end_ms'] > played_ms:
            break
        piece = span['text']
        room = limit - len(text.encode())
        raw = piece.encode()
        if len(raw) <= room:
            text += piece
            continue
        take = room
        while take and (raw[take - 1] & 0xC0) == 0x80:
            take -= 1
        if take and (raw[take - 1] & 0x80):
            while take and (raw[take - 1] & 0xC0) != 0xC0:
                take -= 1
            take -= 1
        text += raw[:max(take, 0)].decode()
        break
    return text


class DuplexEngine:
    """In-memory duplex body. Callers supply virtual milliseconds."""

    RENDER_LATENCY_MS = 20

    def __init__(self, barge='stop', render_latency_ms=RENDER_LATENCY_MS):
        if barge not in ('stop', 'duck'):
            raise ValueError('barge must be stop or duck')
        self.barge = barge
        self.render_latency_ms = render_latency_ms
        self.reset()

    def reset(self):
        self.utterances = {}
        self.closed = set()
        self.discarded = 0
        self.output = 'idle'
        self.terminal = None
        self.stream = None
        self.item = None
        self.chunk = None
        self.played_ms = 0
        self.position_ms = 0
        self.spans = []
        self.pending_play_at = None
        self.events = []
        self.journal = []
        self.expression = 'neutral'
        self.intensity = 0
        self.expression_until = 0
        self.work_epoch = 1
        self.pending_work = []
        self.remembered_calls = {}
        self.work_done = []
        self.capture_mode = None

    def snapshot(self):
        return deepcopy({
            'barge': self.barge,
            'render_latency_ms': self.render_latency_ms,
            'utterances': self.utterances,
            'closed': sorted(self.closed),
            'discarded': self.discarded,
            'output': self.output,
            'terminal': self.terminal,
            'stream': self.stream,
            'item': self.item,
            'chunk': self.chunk,
            'played_ms': self.played_ms,
            'position_ms': self.position_ms,
            'spans': self.spans,
            'pending_play_at': self.pending_play_at,
            'events': self.events,
            'journal': self.journal,
            'expression': self.expression,
            'intensity': self.intensity,
            'expression_until': self.expression_until,
            'work_epoch': self.work_epoch,
            'pending_work': self.pending_work,
            'remembered_calls': self.remembered_calls,
            'work_done': self.work_done,
            'capture_mode': self.capture_mode,
        })

    def restore(self, snapshot):
        data = deepcopy(snapshot)
        self.barge = data['barge']
        self.render_latency_ms = data['render_latency_ms']
        self.utterances = data['utterances']
        self.closed = set(data['closed'])
        self.discarded = data['discarded']
        self.output = data['output']
        self.terminal = data['terminal']
        self.stream = data['stream']
        self.item = data['item']
        self.chunk = data['chunk']
        self.played_ms = data['played_ms']
        self.position_ms = data['position_ms']
        self.spans = data['spans']
        self.pending_play_at = data['pending_play_at']
        self.events = data['events']
        self.journal = data['journal']
        self.expression = data['expression']
        self.intensity = data['intensity']
        self.expression_until = data['expression_until']
        self.work_epoch = data['work_epoch']
        self.pending_work = data['pending_work']
        self.remembered_calls = data['remembered_calls']
        self.work_done = data['work_done']
        self.capture_mode = data['capture_mode']

    def _emit(self, at_ms, name, payload):
        event = {'at_ms': at_ms, 'name': name, 'payload': payload}
        self.events.append(event)
        self.journal.append(event)
        return event

    def _position(self, at_ms, state=None):
        state = state or self.output
        return self._emit(at_ms, 'speech.position', {
            'stream': self.stream, 'chunk': self.chunk, 'item': self.item,
            'played_ms': self.played_ms, 'position_ms': self.position_ms, 'state': state,
        })

    def speak(self, at_ms, stream, chunk, item, text, final=True, spans=None):
        if self.stream and self.output not in ('idle', 'finished', 'stopped'):
            raise ValueError('conflict: stream already active')
        self.stream = stream
        self.chunk = chunk
        self.item = item
        self.spans = list(spans or [])
        self.played_ms = 0
        self.position_ms = 0
        self.output = 'playing'
        self.terminal = None
        self.pending_play_at = at_ms + self.render_latency_ms
        return self.tick(at_ms)

    def tick(self, now_ms):
        emitted = []
        if self.pending_play_at is not None and now_ms >= self.pending_play_at:
            self.pending_play_at = None
            emitted.append(self._position(now_ms, 'playing'))
        if self.expression_until and now_ms >= self.expression_until:
            self.expression = 'neutral'
            self.intensity = 0
            self.expression_until = 0
        return emitted

    def feed(self, at_ms, kind, **step):
        self.tick(at_ms)
        emitted = []
        if kind == 'play':
            self.stream = step['stream']
            self.chunk = step.get('chunk', 0)
            self.item = step['item']
            self.played_ms = step.get('played_ms', 0)
            self.position_ms = step.get('position_ms', self.played_ms)
            self.output = 'playing'
            self.terminal = None
            self.pending_play_at = None
            emitted.append(self._position(at_ms, 'playing'))
        elif kind == 'start':
            uid = step['utterance']
            if uid in self.utterances:
                raise ValueError('utterance already started')
            self.utterances[uid] = 0
            emitted.append(self._emit(at_ms, 'speech.start', {
                'utterance': uid, 'position_ms': step.get('position_ms', 0),
            }))
            if self.pending_work and self.output not in ('playing', 'ducked'):
                emitted.extend(self.cancel_work(at_ms))
        elif kind in ('partial', 'end'):
            uid = step['utterance']
            if uid not in self.utterances:
                raise ValueError('unknown utterance')
            if uid in self.closed or step['revision'] <= self.utterances[uid]:
                self.discarded += 1
                return []
            self.utterances[uid] = step['revision']
            payload = {'utterance': uid, 'revision': step['revision'], 'text': step['text']}
            if kind == 'end':
                self.closed.add(uid)
                payload['position_ms'] = step.get('position_ms', 0)
            emitted.append(self._emit(at_ms, 'speech.' + kind, payload))
        elif kind == 'barge_in':
            if step['stream'] != self.stream:
                raise ValueError('barge stream mismatch')
            reflex = 'stopped' if self.barge == 'stop' else 'ducked'
            self.played_ms = step['played_ms']
            self.position_ms = step.get('position_ms', step['played_ms'])
            self.output = reflex
            emitted.append(self._emit(at_ms, 'speech.barge_in', {
                'utterance': step['utterance'], 'stream': self.stream, 'item': step['item'],
                'played_ms': self.played_ms, 'position_ms': self.position_ms, 'reflex': reflex,
            }))
            emitted.append(self._position(at_ms, reflex))
            if reflex == 'stopped':
                self.terminal = 'cancelled'
                emitted.append(self._emit(at_ms, 'action.outcome', {
                    'id': step['item'], 'correlation_id': step['item'], 'status': 'cancelled',
                    'error': {'code': 'cancelled', 'message': 'No grant', 'retryable': False},
                }))
            emitted.extend(self.cancel_work(at_ms))
        elif kind == 'finish':
            self.played_ms = step['played_ms']
            self.position_ms = step.get('position_ms', step['played_ms'])
            self.output = 'finished'
            self.terminal = 'succeeded'
            self.pending_play_at = None
            emitted.append(self._position(at_ms, 'finished'))
            emitted.append(self._emit(at_ms, 'action.outcome', {
                'id': step['item'], 'correlation_id': step['item'], 'status': 'succeeded',
                'result': {},
            }))
        else:
            raise ValueError(kind)
        return emitted

    def run_timeline(self, timeline):
        self.reset()
        self.barge = timeline['config']['barge']
        for step in timeline['steps']:
            payload = {k: v for k, v in step.items() if k not in ('input', 'at_ms')}
            self.feed(step['at_ms'], step['input'], **payload)
        names = [e['name'] for e in self.events]
        return {
            'events': names,
            'discarded': self.discarded,
            'output': self.output,
            'terminal': self.terminal,
            'journal': deepcopy(self.events),
        }

    def heard(self, item):
        if item != self.item:
            return None
        return {
            'item': item,
            'played_ms': self.played_ms,
            'text': heard_prefix(self.spans, self.played_ms),
        }

    def set_expression(self, now_ms, expression, intensity, duration_ms):
        if duration_ms == 0:
            self.expression = 'neutral'
            self.intensity = 0
            self.expression_until = 0
            return {}
        self.expression = expression
        self.intensity = intensity
        self.expression_until = now_ms + duration_ms
        return {}

    def expression_state(self, now_ms):
        self.tick(now_ms)
        remaining = max(0, self.expression_until - now_ms) if self.expression_until else 0
        return {
            'expression': self.expression,
            'intensity': self.intensity,
            'remaining_ms': remaining,
        }

    def stage_work(self, at_ms, turn, call, name, arguments):
        key = (turn, call)
        if key in self.remembered_calls:
            return []
        if len(self.remembered_calls) >= 128:
            raise ValueError('resource_exhausted')
        intent = {
            'turn': turn, 'call': call, 'epoch': self.work_epoch,
            'name': name, 'arguments': arguments,
        }
        self.remembered_calls[key] = intent
        self.pending_work.append(intent)
        return [self._emit(at_ms, 'speech.work_intent', intent)]

    def cancel_work(self, at_ms):
        emitted = []
        self.work_epoch += 1
        pending = list(self.pending_work)
        self.pending_work = []
        for intent in pending:
            done = {
                'turn': intent['turn'], 'call': intent['call'], 'epoch': intent['epoch'],
                'status': 'cancelled',
                'value': {'code': 'cancelled', 'message': 'No grant', 'retryable': False},
            }
            self.work_done.append(done)
            emitted.append(self._emit(at_ms, 'speech.work_done', done))
        return emitted

    def work_state(self):
        return {'epoch': self.work_epoch, 'pending': len(self.pending_work)}

    def work_result(self, turn, call, epoch, value):
        for index, intent in enumerate(self.pending_work):
            if intent['call'] != call:
                continue
            if intent['turn'] != turn or intent['epoch'] != epoch:
                raise ValueError('conflict')
            done = {
                'turn': turn, 'call': call, 'epoch': epoch,
                'status': 'succeeded', 'value': value,
            }
            self.pending_work.pop(index)
            self.work_done.append(done)
            return done
        raise KeyError('not_found')
