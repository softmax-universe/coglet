"""Wire regressions through the real Surface.receive path (no harness shim).

Item identity must be the admitted action ID, outcomes must carry the admitted
correlation, speech.stop must cancel queued/playing speak actions, and fenced
callbacks must never emit after stop.
"""
import time

import pytest

from coglet_surface.codec import decode, encode
from coglet_surface.surface import WIRE

from coglet_sim.host import COGLET, HELLO_ID, bind_session

STREAM = '2' * 32


class Wire:
    def __init__(self, case, tmp_path):
        self.surface, self.device, configured = bind_session(case, tmp_path / 'ledger')
        self.messages = []
        self.surface.open(HELLO_ID)
        self.send('hello_reply', {'selection': {'version': '0.1'}, 'correlation_id': HELLO_ID})
        configured()
        self.pump()

    def send(self, kind, body):
        self.surface.receive(encode(WIRE, {'type': kind, 'body': body}))
        self.pump()

    def pump(self):
        self.device.tick()
        self.messages.extend(decode(WIRE, raw) for raw in self.surface.drain())

    def act(self, aid, capability, args, correlation=None, deadline=4_000_000):
        self.send('act', {'grant_epoch': 1, 'action': {
            'id': aid, 'coglet': COGLET, 'capability': capability, 'args': args,
            'deadline': deadline, 'correlation_id': correlation or aid}})

    def speak(self, aid, chunk, text, final, correlation=None, stream=STREAM):
        self.act(aid, 'speech.speak_chunk', dict(stream=stream, chunk=chunk, text=text, final=final),
                 correlation)

    def read(self, item, correlation):
        self.send('read', {'coglet': COGLET, 'capability': 'speech.heard', 'args': {'item': item},
                           'grant_epoch': 1, 'deadline': 4_000_000, 'correlation_id': correlation})

    def settle(self, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.pump()
            time.sleep(0.002)

    def until(self, predicate, timeout=2.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.pump()
            if predicate(self):
                return
            time.sleep(0.002)
        raise AssertionError(f'timed out: {self.kinds()}')

    def events(self, kind=None):
        return [m['body'] for m in self.messages
                if m['type'] == 'event' and (kind is None or m['body']['kind'] == kind)]

    def outcomes(self):
        return {e['payload']['id']: e['payload'] for e in self.events('action.outcome')}

    def accepts(self):
        return {m['body']['id']: m['body'] for m in self.messages if m['type'] == 'accept'}

    def kinds(self):
        return [m['body']['kind'] if m['type'] == 'event' else m['type'] for m in self.messages]

    def pending(self):
        return sorted(aid for _, aid, _ in self.surface._pending())

    def close(self):
        self.surface.close()


@pytest.fixture
def wire(tmp_path):
    opened = []

    def make(case=''):
        directory = tmp_path / str(len(opened))
        directory.mkdir(mode=0o700)
        opened.append(Wire(case, directory))
        return opened[-1]
    yield make
    for w in opened:
        w.close()


@pytest.mark.parametrize('aid, correlation', [
    ('1' * 32, '1' * 32),  # fixture identity control
    ('9' * 32, '9' * 32),  # reviewer probe: varied action ID and correlation
    ('a' * 32, '5' * 32),  # correlation differs from action ID
])
def test_heard_truncation_items_are_the_admitted_action_id(wire, aid, correlation):
    w = wire('heard-truncation')
    w.speak(aid, 0, 'Hello world', True, correlation)
    assert w.accepts()[aid]['accepted'] is True
    w.until(lambda w: aid in w.outcomes())
    positions = [e['payload'] for e in w.events('speech.position')]
    barge = [e['payload'] for e in w.events('speech.barge_in')]
    assert [p['item'] for p in positions] == [aid, aid]
    assert [p['state'] for p in positions] == ['playing', 'stopped']
    assert barge == [{'utterance': '4' * 32, 'stream': STREAM, 'item': aid, 'played_ms': 1200,
                      'position_ms': 1200, 'reflex': 'stopped'}]
    outcome = w.outcomes()[aid]
    assert outcome['status'] == 'cancelled'
    assert outcome['correlation_id'] == correlation
    assert w.pending() == []
    w.read(aid, 'c' * 32)
    reply = [m['body'] for m in w.messages if m['type'] == 'read_reply'][-1]
    assert reply['ok'] is True
    assert reply['value'] == {'item': aid, 'played_ms': 1200, 'text': 'Hello'}
    w.read('1' * 32 if aid != '1' * 32 else '8' * 32, 'd' * 32)
    reply = [m['body'] for m in w.messages if m['type'] == 'read_reply'][-1]
    assert reply['ok'] is False and reply['error']['code'] == 'not_found'


def test_stop_before_playback_cancels_the_pending_speak(wire):
    w = wire()
    w.speak('1' * 32, 0, 'hello', True)
    w.act('3' * 32, 'speech.stop', {'stream': STREAM})
    outcomes = w.outcomes()
    assert outcomes['1' * 32]['status'] == 'cancelled'
    assert outcomes['3' * 32]['status'] == 'succeeded'
    # Cancellation precedes stop success: the stream is silent when stop succeeds.
    order = [e['payload']['id'] for e in w.events('action.outcome')]
    assert order == ['1' * 32, '3' * 32]
    assert w.pending() == []
    w.settle(0.15)  # the queued render callback is fenced: nothing ever plays
    assert w.events('speech.position') == []
    w.act('5' * 32, 'speech.stop', {'stream': STREAM})  # idempotent on a known stopped stream
    assert w.outcomes()['5' * 32]['status'] == 'succeeded'
    w.speak('6' * 32, 1, 'more', True)  # a stopped stream cannot resume
    assert w.accepts()['6' * 32]['error']['code'] == 'conflict'


def test_stop_during_playback_cancels_playing_and_queued_chunks(wire):
    w = wire()
    w.speak('1' * 32, 0, 'one two three four', False)
    w.speak('7' * 32, 1, 'five six', True)
    w.until(lambda w: w.events('speech.position'))
    w.act('3' * 32, 'speech.stop', {'stream': STREAM})
    positions = [e['payload'] for e in w.events('speech.position')]
    assert [(p['item'], p['state']) for p in positions] == [('1' * 32, 'playing'), ('1' * 32, 'stopped')]
    outcomes = w.outcomes()
    assert outcomes['1' * 32]['status'] == 'cancelled'
    assert outcomes['7' * 32]['status'] == 'cancelled'
    assert outcomes['3' * 32]['status'] == 'succeeded'
    assert w.pending() == []
    before = len(w.messages)
    w.settle(0.25)  # finish/render callbacks of the stopped item are fenced
    assert [m for m in w.messages[before:] if m['type'] == 'event'] == []


def test_stop_after_completion_leaves_finished_action_unchanged(wire):
    w = wire()
    w.speak('1' * 32, 0, 'hi', True, '6' * 32)
    w.until(lambda w: '1' * 32 in w.outcomes())
    assert w.outcomes()['1' * 32] == {'id': '1' * 32, 'correlation_id': '6' * 32,
                                       'status': 'succeeded', 'result': {}}
    positions = [e['payload'] for e in w.events('speech.position')]
    assert [(p['item'], p['state'], p['played_ms']) for p in positions] == [
        ('1' * 32, 'playing', 0), ('1' * 32, 'finished', 100)]
    w.act('3' * 32, 'speech.stop', {'stream': STREAM})
    assert w.outcomes()['3' * 32]['status'] == 'succeeded'
    assert len(w.events('action.outcome')) == 2  # no second outcome for the finished speak
    assert w.events('speech.position')[-1]['payload']['state'] == 'finished'


def test_unknown_stream_stop_is_not_found_and_other_stream_conflicts(wire):
    w = wire()
    w.act('3' * 32, 'speech.stop', {'stream': 'f' * 32})
    assert w.accepts()['3' * 32]['error']['code'] == 'not_found'
    w.speak('1' * 32, 0, 'hello there', True)
    w.speak('4' * 32, 0, 'other', True, stream='e' * 32)
    assert w.accepts()['4' * 32]['error']['code'] == 'conflict'
    w.speak('5' * 32, 2, 'gap', True)
    assert w.accepts()['5' * 32]['error']['code'] == 'conflict'


def test_chunks_play_in_order_with_cumulative_position(wire):
    w = wire()
    w.speak('a' * 32, 0, 'ab', False)
    w.speak('b' * 32, 1, 'cde', True)
    w.until(lambda w: 'b' * 32 in w.outcomes())
    positions = [e['payload'] for e in w.events('speech.position')]
    assert [(p['item'], p['chunk'], p['state'], p['played_ms'], p['position_ms']) for p in positions] == [
        ('a' * 32, 0, 'playing', 0, 0), ('a' * 32, 0, 'finished', 100, 100),
        ('b' * 32, 1, 'playing', 0, 100), ('b' * 32, 1, 'finished', 150, 250)]
    assert [w.outcomes()[i]['status'] for i in ('a' * 32, 'b' * 32)] == ['succeeded', 'succeeded']


def test_revocation_fences_scheduled_playback(wire):
    w = wire()
    w.speak('1' * 32, 0, 'a long sentence to play', True)
    w.until(lambda w: w.events('speech.position'))
    w.surface.grant([])  # revocation: native stop + cancellation outcome
    w.pump()
    assert w.outcomes()['1' * 32]['status'] == 'cancelled'
    before = len(w.messages)
    w.settle(0.2)
    assert [m for m in w.messages[before:] if m['type'] == 'event'] == []
