"""Deterministic duplex-audio engine: no waveform, no provider, no real clock.

Playback, barge-in, capture normalization and heard truncation follow
spec/capabilities/speech.md and the scripted timelines in
spec/fixtures/speech/timelines.json. Callers supply virtual media
milliseconds; positions are scripted or derived from the simulated voice's
known media duration, never inferred from wall time or text-duration ratios.

Every speak item (wire action ID or provider-native ID) has its own record, so
heard history of a finished item survives later items and an interrupted item
never extends. Unknown alignment (no spans) always yields empty heard text.
"""
from __future__ import annotations

from copy import deepcopy

from coglet_surface.codec import encode_profile

CANCELLED = {'code': 'cancelled', 'message': 'No grant', 'retryable': False}
WORK_ARGUMENT_BYTES = 8192
WORK_RESULT_BYTES = 32000


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
        # Cut at the byte budget, dropping only an incomplete trailing character.
        text += raw[:room].decode('utf-8', errors='ignore')
        break
    return text


class DuplexEngine:
    """In-memory duplex body. Callers supply virtual milliseconds."""

    RENDER_LATENCY_MS = 20
    LIVE = ('queued', 'playing', 'ducked')

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
        self.items = {}
        self.streams = {}
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

    FIELDS = ('barge', 'render_latency_ms', 'utterances', 'discarded', 'output', 'terminal',
              'stream', 'item', 'chunk', 'played_ms', 'position_ms', 'spans', 'pending_play_at',
              'items', 'streams', 'events', 'journal', 'expression', 'intensity',
              'expression_until', 'work_epoch', 'pending_work', 'remembered_calls',
              'work_done', 'capture_mode')

    def snapshot(self):
        data = {name: getattr(self, name) for name in self.FIELDS}
        data['closed'] = sorted(self.closed)
        # call id -> intent. Dedup is per call within the work epoch, not per turn.
        data['remembered_calls'] = [[k, v] for k, v in self.remembered_calls.items()]
        return deepcopy(data)

    def restore(self, snapshot):
        data = deepcopy(snapshot)
        for name in self.FIELDS:
            setattr(self, name, data[name])
        self.closed = set(data['closed'])
        self.remembered_calls = {k: v for k, v in data['remembered_calls']}

    def _emit(self, at_ms, name, payload):
        event = {'at_ms': at_ms, 'name': name, 'payload': payload}
        self.events.append(event)
        self.journal.append(event)
        return event

    # Playback table -----------------------------------------------------

    def _register(self, item, stream, chunk, spans=(), duration_ms=None, state='queued', owner=None):
        if item in self.items:
            raise ValueError('conflict: item already known')
        record = self.streams.setdefault(stream, {'items': [], 'stopped': False, 'final': False,
                                                  'position_ms': 0, 'owner': owner})
        record['items'].append(item)
        self.items[item] = {
            'stream': stream, 'chunk': chunk, 'spans': list(spans), 'owner': owner,
            'duration_ms': duration_ms, 'played_ms': 0, 'base_ms': None, 'state': state,
        }
        return self.items[item]

    def _set_current(self, item):
        record = self.items[item]
        self.item, self.stream, self.chunk = item, record['stream'], record['chunk']
        self.played_ms = record['played_ms']
        self.position_ms = record['base_ms'] + record['played_ms']
        self.spans = record['spans']

    def _position(self, at_ms, state=None):
        state = state or self.output
        return self._emit(at_ms, 'speech.position', {
            'stream': self.stream, 'chunk': self.chunk, 'item': self.item,
            'played_ms': self.played_ms, 'position_ms': self.position_ms, 'state': state,
        })

    def _outcome(self, at_ms, item, status):
        payload = {'id': item, 'status': status}
        if status == 'succeeded':
            payload['result'] = {}
        else:
            payload['error'] = dict(CANCELLED)
        return self._emit(at_ms, 'action.outcome', payload)

    def enqueue(self, stream, chunk, item, spans=(), duration_ms=None, final=False, owner=None):
        """Admit one speak chunk. Media duration comes from the simulated voice."""
        known = self.streams.get(stream)
        if known and (known['stopped'] or known['final']):
            raise ValueError('conflict: stream closed')
        if known and len(known['items']) != chunk or not known and chunk != 0:
            raise ValueError('conflict: noncontiguous chunk')
        if any(s != stream and self.stream_open(s) for s in self.streams):
            raise ValueError('conflict: another stream is open')
        self._register(item, stream, chunk, spans, duration_ms, 'queued', owner)
        if final:
            self.streams[stream]['final'] = True

    def enqueue_backchannel(self, stream, item, spans=(), duration_ms=None, owner=None):
        """Admit a backchannel as its own chunk-0, implicitly final item.

        Fails if the stream id is already known or another stream is active.
        The item id is the admitted action id; one terminal outcome follows render.
        """
        if stream in self.streams:
            raise ValueError('conflict: stream id is not reusable')
        if any(self.stream_open(s) for s in self.streams):
            raise ValueError('conflict: speaker is busy')
        self._register(item, stream, 0, spans, duration_ms, 'queued', owner)
        self.streams[stream]['final'] = True
        self.streams[stream]['backchannel'] = True

    def stream_open(self, stream):
        record = self.streams.get(stream)
        if not record or record['stopped']:
            return False
        return not record['final'] or any(self.items[i]['state'] in self.LIVE for i in record['items'])

    def next_queued(self, stream):
        for item in self.streams.get(stream, {'items': []})['items']:
            state = self.items[item]['state']
            if state in ('playing', 'ducked'):
                return None
            if state == 'queued':
                return item
        return None

    def render(self, at_ms, item):
        """First rendered audio for a queued item: playing position at played 0."""
        record = self.items[item]
        if record['state'] != 'queued':
            return []
        stream = self.streams[record['stream']]
        record['state'] = 'ducked' if self.output == 'ducked' and self.stream == record['stream'] else 'playing'
        record['base_ms'] = stream['position_ms']
        self.output = record['state']
        self.terminal = None
        self._set_current(item)
        return [self._position(at_ms, record['state'])]

    def complete_item(self, at_ms, item):
        """Item rendered to its known media end: finished position + success."""
        record = self.items[item]
        if record['state'] not in ('playing', 'ducked'):
            return []
        record['played_ms'] = record['duration_ms']
        record['state'] = 'finished'
        self.streams[record['stream']]['position_ms'] = record['base_ms'] + record['played_ms']
        self._set_current(item)
        self.output = 'finished'
        self.terminal = 'succeeded'
        return [self._position(at_ms, 'finished'), self._outcome(at_ms, item, 'succeeded')]

    def stop_stream(self, at_ms, stream):
        """Coglet speech.stop: silence the stream, cancel queued/playing items."""
        record = self.streams[stream]
        record['stopped'] = True
        emitted = []
        for item in record['items']:
            state = self.items[item]['state']
            if state in ('playing', 'ducked'):
                self.items[item]['state'] = 'stopped'
                self._set_current(item)
                self.output = 'stopped'
                self.terminal = 'cancelled'
                emitted.append(self._position(at_ms, 'stopped'))
                emitted.append(self._outcome(at_ms, item, 'cancelled'))
            elif state == 'queued':
                self.items[item]['state'] = 'cancelled'
                emitted.append(self._outcome(at_ms, item, 'cancelled'))
        return emitted

    def halt(self):
        """Native off-switch (revocation, deadline, disconnect): silence everything.

        Outcomes for admitted work come from the surface core, not the engine."""
        for record in self.streams.values():
            record['stopped'] = True
        for record in self.items.values():
            if record['state'] in ('playing', 'ducked'):
                record['state'] = 'stopped'
            elif record['state'] == 'queued':
                record['state'] = 'cancelled'
        if self.output in ('playing', 'ducked'):
            self.output = 'stopped'
        self.pending_play_at = None

    def forget_heard(self, items):
        """Remove heard records (owner revocation, speech.md heard retention).

        The playback-table entry stays as a tombstone so item identity, stream
        closure and chunk contiguity keep fencing late callbacks and replays;
        only the heard projection (aligned spans, played prefix) is erased."""
        for item in items:
            record = self.items[item]
            record['heard_removed'] = True
            record['spans'] = []
        if self.item in items:
            self.spans = []

    def active_item(self):
        for item, record in self.items.items():
            if record['state'] in ('playing', 'ducked'):
                return item
        return None

    # Timeline inputs ----------------------------------------------------

    def tick(self, now_ms):
        """Tick a non-accelerated standalone engine sharing one clock."""
        self.tick_expression(now_ms)
        return self.tick_media(now_ms)

    def tick_expression(self, now_ms):
        """Native monotonic milliseconds, never accelerated media time."""
        if self.expression_until and now_ms >= self.expression_until:
            self.reset_expression()

    def tick_media(self, now_ms):
        emitted = []
        if self.pending_play_at is not None and now_ms >= self.pending_play_at:
            self.pending_play_at = None
            emitted.append(self._position(now_ms, 'playing'))
        return emitted

    def feed(self, at_ms, kind, **step):
        self.tick_media(at_ms)
        emitted = []
        if kind == 'play':
            item = step['item']
            if item not in self.items:
                self._register(item, step['stream'], step.get('chunk', 0), step.get('spans', ()),
                               step.get('duration_ms'), 'queued', step.get('owner'))
            record = self.items[item]
            record['state'] = 'playing'
            record['played_ms'] = step.get('played_ms', 0)
            record['base_ms'] = step.get('position_ms', record['played_ms']) - record['played_ms']
            self.output = 'playing'
            self.terminal = None
            self.pending_play_at = None
            self._set_current(item)
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
            text = step['text']
            # Byte cap is additional to the schema's code-point limit. Oversized
            # transcripts are discarded, not forwarded and not used to reopen.
            if uid in self.closed or step['revision'] <= self.utterances[uid] or len(text.encode()) > 16384:
                self.discarded += 1
                return []
            self.utterances[uid] = step['revision']
            payload = {'utterance': uid, 'revision': step['revision'], 'text': text}
            if kind == 'end':
                self.closed.add(uid)
                payload['position_ms'] = step.get('position_ms', 0)
            emitted.append(self._emit(at_ms, 'speech.' + kind, payload))
        elif kind == 'barge_in':
            item = step['item']
            record = self.items.get(item)
            if record is None or record['stream'] != step['stream'] or record['state'] not in ('playing', 'ducked'):
                raise ValueError('barge item is not the active output')
            played = step['played_ms']
            if played < record['played_ms'] or (record['duration_ms'] is not None
                                                and played > record['duration_ms']):
                raise ValueError('barge position regressed or exceeds media')
            reflex = 'stopped' if self.barge == 'stop' else 'ducked'
            record['played_ms'] = played
            if 'position_ms' in step:
                record['base_ms'] = step['position_ms'] - played
            record['state'] = reflex
            self._set_current(item)
            self.output = reflex
            emitted.append(self._emit(at_ms, 'speech.barge_in', {
                'utterance': step['utterance'], 'stream': self.stream, 'item': item,
                'played_ms': self.played_ms, 'position_ms': self.position_ms, 'reflex': reflex,
            }))
            emitted.append(self._position(at_ms, reflex))
            if reflex == 'stopped':
                self.terminal = 'cancelled'
                stream = self.streams[record['stream']]
                stream['stopped'] = True
                stream['position_ms'] = self.position_ms
                emitted.append(self._outcome(at_ms, item, 'cancelled'))
                for later in stream['items']:
                    if self.items[later]['state'] == 'queued':
                        self.items[later]['state'] = 'cancelled'
                        emitted.append(self._outcome(at_ms, later, 'cancelled'))
            emitted.extend(self.cancel_work(at_ms))
        elif kind == 'finish':
            record = self.items[step['item']]
            if record['state'] not in ('playing', 'ducked'):
                raise ValueError('finish of inactive item')
            record['duration_ms'] = step['played_ms']
            if 'position_ms' in step:
                record['base_ms'] = step['position_ms'] - step['played_ms']
            emitted.extend(self.complete_item(at_ms, step['item']))
        else:
            raise ValueError(kind)
        return emitted

    def run_timeline(self, timeline, *, upto=None):
        """Run a frozen timeline from reset (or only its first `upto` steps)."""
        self.reset()
        self.barge = timeline['config']['barge']
        return self.continue_timeline(timeline, 0, upto)

    def continue_timeline(self, timeline, start, upto=None):
        for step in timeline['steps'][start:upto]:
            payload = {k: v for k, v in step.items() if k not in ('input', 'at_ms')}
            self.feed(step['at_ms'], step['input'], **payload)
        return self.result()

    def result(self):
        return {
            'events': [e['name'] for e in self.events],
            'discarded': self.discarded,
            'output': self.output,
            'terminal': self.terminal,
            'journal': deepcopy(self.events),
        }

    def heard(self, item):
        """speech.heard record for exactly that item, or None when unknown."""
        record = self.items.get(item)
        if record is None or record.get('heard_removed'):
            return None
        return {
            'item': item,
            'played_ms': record['played_ms'],
            'text': heard_prefix(record['spans'], record['played_ms']),
        }

    def reset_expression(self):
        """Kill, revocation and disconnect return the renderer to neutral."""
        self.expression = 'neutral'
        self.intensity = 0
        self.expression_until = 0

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
        self.tick_expression(now_ms)
        remaining = max(0, self.expression_until - now_ms) if self.expression_until else 0
        return {
            'expression': self.expression,
            'intensity': self.intensity,
            'remaining_ms': remaining,
        }

    def stage_work(self, at_ms, turn, call, name, arguments):
        """Stage one work_intent. Identical (call) repeats are ignored.

        A repeated call with a different turn, name or arguments conflicts and
        is not staged. Oversized argument CBOR is not staged and emits nothing.
        """
        previous = self.remembered_calls.get(call)
        if previous is not None:
            same = (previous['turn'], previous['name'], previous['arguments']) == (turn, name, arguments)
            if same:
                return []
            raise ValueError('conflict')
        if len(encode_profile(arguments)) > WORK_ARGUMENT_BYTES or len(self.remembered_calls) >= 128:
            if len(self.remembered_calls) >= 128 and len(encode_profile(arguments)) <= WORK_ARGUMENT_BYTES:
                raise ValueError('resource_exhausted')
            return []
        if len(self.pending_work) >= 4:
            raise ValueError('resource_exhausted')
        intent = {
            'turn': turn, 'call': call, 'epoch': self.work_epoch,
            'name': name, 'arguments': arguments,
        }
        self.remembered_calls[call] = intent
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
                'status': 'cancelled', 'value': dict(CANCELLED),
            }
            self.work_done.append(done)
            emitted.append(self._emit(at_ms, 'speech.work_done', done))
        return emitted

    def work_state(self):
        return {'epoch': self.work_epoch, 'pending': len(self.pending_work)}

    def work_result(self, turn, call, epoch, status, value):
        """Accept one live tuple. status/value are the caller's, not implied success.

        Unknown, cancelled or old-epoch tuples are not_found. A mismatched turn
        conflicts and leaves the call pending. The delivery action's own outcome
        is separate and may succeed after a failed work_done.
        """
        if status not in ('succeeded', 'failed'):
            raise ValueError('invalid work status')
        if len(encode_profile(value)) > WORK_RESULT_BYTES:
            raise ValueError('invalid')
        for index, intent in enumerate(self.pending_work):
            if intent['call'] != call:
                continue
            if intent['epoch'] != epoch:
                raise KeyError('not_found')
            if intent['turn'] != turn:
                raise ValueError('conflict')
            done = {
                'turn': turn, 'call': call, 'epoch': epoch,
                'status': status, 'value': value,
            }
            self.pending_work.pop(index)
            self.work_done.append(done)
            return done
        raise KeyError('not_found')
