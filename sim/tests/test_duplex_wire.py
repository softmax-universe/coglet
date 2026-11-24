"""Wire regressions through the real Surface.receive path (no harness shim).

Item identity must be the admitted action ID, outcomes must carry the admitted
correlation, speech.stop must cancel queued/playing speak actions, and fenced
callbacks must never emit after stop.
"""
import time

import pytest

from coglet_surface.codec import decode, encode
from coglet_surface.surface import WIRE

from coglet_sim.host import COGLET, CUE_MS, HELLO_ID, TURN, CALL, bind_session, grant_entries, operations_for

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

    def act(self, aid, capability, args, correlation=None, deadline=4_000_000, epoch=1):
        self.send('act', {'grant_epoch': epoch, 'action': {
            'id': aid, 'coglet': COGLET, 'capability': capability, 'args': args,
            'deadline': deadline, 'correlation_id': correlation or aid}})

    def speak(self, aid, chunk, text, final, correlation=None, stream=STREAM, epoch=1, deadline=4_000_000):
        self.act(aid, 'speech.speak_chunk', dict(stream=stream, chunk=chunk, text=text, final=final),
                 correlation, deadline=deadline, epoch=epoch)

    def read(self, item, correlation, epoch=1):
        self.query('speech.heard', {'item': item}, correlation, epoch)

    def query(self, capability, args, correlation, epoch=1):
        self.send('read', {'coglet': COGLET, 'capability': capability, 'args': args,
                           'grant_epoch': epoch, 'deadline': 4_000_000, 'correlation_id': correlation})

    def reply(self):
        return [m['body'] for m in self.messages if m['type'] == 'read_reply'][-1]

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

    def caps(self):
        return [m['body'] for m in self.messages if m['type'] == 'capabilities'][-1]['entries']

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


def ok_value(reply, correlation):
    assert reply['correlation_id'] == correlation and reply['ok'] is True, reply
    return reply['value']


@pytest.mark.parametrize('revoke_first', [True, False], ids=['revoke-then-regrant', 'direct-regrant'])
def test_heard_records_are_removed_on_owner_revocation(wire, revoke_first):
    """speech.md: heard records are removed on owner revocation; a same-owner
    regrant is a new epoch and must not read items admitted under the old one."""
    w = wire('heard-truncation')
    old, new = '9' * 32, 'b' * 32
    w.speak(old, 0, 'Hello world', True)
    w.until(lambda w: old in w.outcomes())
    w.read(old, 'c' * 32)  # pre-revocation control: the owner reads its heard prefix
    assert ok_value(w.reply(), 'c' * 32) == {'item': old, 'played_ms': 1200, 'text': 'Hello'}

    entries = grant_entries(operations_for('heard-truncation'))
    if revoke_first:
        w.surface.grant([])
    w.surface.grant(entries)
    w.pump()
    epoch = 3 if revoke_first else 2
    assert w.surface._get('epoch') == epoch

    w.read(old, 'd' * 32, epoch=epoch)  # same owner, new epoch: record is gone
    reply = w.reply()
    assert reply['ok'] is False and reply['error']['code'] == 'not_found'
    assert w.device.engine.heard(old) is None  # removed, not merely hidden
    assert w.device.engine.items[old]['state'] == 'stopped'  # tombstone kept
    w.read(old, 'e' * 32)  # stale epoch is not admitted either
    assert w.reply()['ok'] is False

    # Tombstone fencing survives: the revoked stream stays closed.
    w.speak('6' * 32, 1, 'more', True, epoch=epoch)
    assert w.accepts()['6' * 32]['error']['code'] == 'conflict'

    # Post-regrant control: a newly admitted item plays and is readable.
    w.speak(new, 0, 'Hello world', True, stream='3' * 32, epoch=epoch)
    assert w.accepts()[new]['accepted'] is True
    w.until(lambda w: new in w.outcomes())
    assert w.outcomes()[new]['status'] == 'succeeded'
    w.read(new, 'f' * 32, epoch=epoch)
    assert ok_value(w.reply(), 'f' * 32) == {'item': new, 'played_ms': 3000, 'text': 'Hello world'}
    w.read(old, '7' * 32, epoch=epoch)  # still gone after new playback
    assert w.reply()['error']['code'] == 'not_found'


# --- Advertised-operation contract (speech.md / expression.md) -----------------

FAILED = {'code': 'unavailable', 'message': 'probe failure', 'retryable': False}
INSPECTED = {'summary': 'Inspected'}
CONTROL = {
    'speech.start', 'speech.partial', 'speech.end', 'speech.position', 'speech.barge_in',
    'speech.config', 'speech.speak_chunk', 'speech.stop', 'speech.backchannel',
    'expression.set_expression', 'expression.state',
}
HANDOFF = CONTROL | {
    'speech.heard', 'speech.work_state', 'speech.work_intent', 'speech.work_done', 'speech.work_result',
}


def advertised(w):
    names = set()
    for entry in w.caps():
        names.update(entry['events'])
        names.update(entry['reads'])
        names.update(entry['actions'])
    return names


def work_payloads(w):
    return [e['payload'] for e in w.events('speech.work_done')]


def test_capabilities_omit_unimplemented_session_ops(wire):
    """Optional provider session ops are not advertised, so a body must not claim them."""
    w = wire()
    assert advertised(w) == CONTROL
    assert 'speech.session' not in {e['name'] for e in w.caps()}
    h = wire('duplicate-call')
    assert advertised(h) == HANDOFF
    m = wire('mode-hands-free')
    assert [e['name'] for e in m.caps()] == ['speech.hands_free']
    assert m.caps()[0]['events'] == m.caps()[0]['reads'] == m.caps()[0]['actions'] == []
    w.act('a' * 32, 'speech.session_open', {'session': 'b' * 32, 'route': 'local'})
    assert w.accepts()['a' * 32]['accepted'] is False
    assert w.accepts()['a' * 32]['error']['code'] == 'permission_denied'


def test_advertised_events_reject_action_admission(wire):
    w = wire()
    w.act('a' * 32, 'speech.start', {'utterance': '1' * 32, 'position_ms': 0})
    assert w.accepts()['a' * 32]['error']['code'] == 'invalid_argument'
    w.act('e' * 32, 'speech.partial', {'utterance': '1' * 32, 'revision': 1, 'text': 'hello'})
    assert w.accepts()['e' * 32]['error']['code'] == 'invalid_argument'
    w.act('f' * 32, 'speech.end', {
        'utterance': '1' * 32, 'revision': 2, 'text': 'hello', 'position_ms': 0})
    assert w.accepts()['f' * 32]['error']['code'] == 'invalid_argument'
    w.act('9' * 32, 'speech.barge_in', {
        'utterance': '1' * 32, 'stream': STREAM, 'item': '1' * 32,
        'played_ms': 0, 'position_ms': 0, 'reflex': 'stopped'})
    assert w.accepts()['9' * 32]['error']['code'] == 'invalid_argument'
    w.act('b' * 32, 'speech.position', {
        'stream': STREAM, 'chunk': 0, 'item': '1' * 32, 'played_ms': 0,
        'position_ms': 0, 'state': 'playing'})
    assert w.accepts()['b' * 32]['error']['code'] == 'invalid_argument'
    h = wire('duplicate-call')
    before = len(h.events('speech.work_intent'))
    h.act('c' * 32, 'speech.work_intent', {
        'turn': TURN, 'call': CALL, 'epoch': 1, 'name': 'game.inspect',
        'arguments': {'request': 'Inspect'}})
    assert h.accepts()['c' * 32]['error']['code'] == 'invalid_argument'
    assert len(h.events('speech.work_intent')) == before == 1


def test_capture_events_accept_valid_and_discard_stale(wire):
    w = wire('late-partial-and-stale-revision')
    w.until(lambda w: w.events('speech.end'))
    w.settle(0.08)  # the late partial is scheduled after end; it must not reopen
    assert [e['payload'] for e in w.events('speech.start')] == [
        {'utterance': '1' * 32, 'position_ms': 0}]
    assert [e['payload'] for e in w.events('speech.partial')] == [
        {'utterance': '1' * 32, 'revision': 2, 'text': 'hello'}]
    assert [e['payload'] for e in w.events('speech.end')] == [
        {'utterance': '1' * 32, 'revision': 3, 'text': 'hello world', 'position_ms': 200}]
    assert w.device.engine.discarded >= 2
    # Oversized transcript is discarded, not forwarded (speech.md byte cap).
    uttered = '8' * 32
    w.device.emit_engine(w.device.engine.feed(400, 'start', utterance=uttered, position_ms=0))
    assert w.device.engine.feed(410, 'partial', utterance=uttered, revision=1, text='é' * 9000) == []
    w.pump()
    assert all(len(e['payload']['text'].encode()) <= 16384 for e in w.events('speech.partial'))


def test_idle_capture_does_not_invent_barge_in(wire):
    w = wire('late-partial-and-stale-revision')
    w.until(lambda w: w.events('speech.start'))
    w.device._capture('barge_in', {'utterance': '1' * 32, 'played_ms': 10}, None)
    w.pump()
    assert w.events('speech.barge_in') == []
    assert w.events('speech.start')  # start still stands; no invented barge item


def test_speech_config_read_and_rejection(wire):
    w = wire()
    w.query('speech.config', {}, 'c' * 32)
    value = ok_value(w.reply(), 'c' * 32)
    assert value['barge'] == 'stop'
    assert value['session_mode'] == 'text'
    assert set(value) == {'stt', 'tts', 'barge', 'session_mode'}
    w.query('speech.config', {'barge': 'duck'}, 'd' * 32)
    assert w.reply()['ok'] is False and w.reply()['error']['code'] == 'invalid_argument'
    w.act('a' * 32, 'speech.config', {})
    assert w.accepts()['a' * 32]['error']['code'] == 'invalid_argument'
    w.surface.grant([])
    w.pump()
    w.query('speech.config', {}, 'e' * 32, epoch=w.surface._get('epoch'))
    assert w.reply()['ok'] is False and w.reply()['error']['code'] == 'permission_denied'


def test_speak_chunk_bounds_idle_close_and_limit(wire):
    w = wire()
    w.speak('3' * 32, 0, 'é' * 2049, True, stream='e' * 32)
    assert w.accepts()['3' * 32]['error']['code'] == 'invalid_argument'
    w.speak('1' * 32, 0, 'hi', False)
    w.until(lambda w: '1' * 32 in w.outcomes())
    w.device.engine.streams[STREAM]['last_admit_us'] = w.surface.now() - 60_000_000
    w.speak('2' * 32, 1, 'more', True)
    assert w.accepts()['2' * 32]['error']['code'] == 'conflict'
    entries = grant_entries(operations_for(''))
    entries[0]['expires'] = w.surface.now() + 300_000_000
    w.surface.grant(entries)
    w.pump()
    epoch = w.surface._get('epoch')
    fresh = 'c' * 32
    for index in range(64):
        aid = f'{index:032x}'
        w.speak(aid, index, 'x', False, stream=fresh,
                deadline=w.surface.now() + 30_000_000, epoch=epoch)
        assert w.accepts()[aid]['accepted'] is True, w.accepts()[aid]
    w.speak('f' * 32, 64, 'x', True, stream=fresh,
            deadline=w.surface.now() + 30_000_000, epoch=epoch)
    assert w.accepts()['f' * 32]['error']['code'] == 'conflict'


@pytest.mark.parametrize('cue', ['ack', 'thinking', 'agree'])
def test_backchannel_plays_each_cue_as_its_own_stream(wire, cue):
    w = wire()
    stream, aid = 'b' * 32, 'a' * 32
    w.act(aid, 'speech.backchannel', {'stream': stream, 'cue': cue})
    assert w.accepts()[aid]['accepted'] is True
    w.until(lambda w: aid in w.outcomes(), timeout=3)
    positions = [e['payload'] for e in w.events('speech.position')]
    assert [(p['item'], p['chunk'], p['stream'], p['state'], p['played_ms']) for p in positions] == [
        (aid, 0, stream, 'playing', 0),
        (aid, 0, stream, 'finished', CUE_MS[cue]),
    ]
    assert w.outcomes()[aid]['status'] == 'succeeded'
    assert w.outcomes()[aid]['result'] == {}
    assert [e['payload']['id'] for e in w.events('action.outcome')] == [aid]
    assert w.pending() == []


def test_backchannel_conflicts_and_stop_fences(wire):
    w = wire()
    w.speak('1' * 32, 0, 'x' * 400, False)  # ~2s of audio; the conflict checks must not outrun it
    w.act('a' * 32, 'speech.backchannel', {'stream': 'b' * 32, 'cue': 'ack'})
    assert w.accepts()['a' * 32]['error']['code'] == 'conflict'  # another stream is active
    w.act('c' * 32, 'speech.backchannel', {'stream': STREAM, 'cue': 'thinking'})
    assert w.accepts()['c' * 32]['error']['code'] == 'conflict'  # stream id already known
    w.act('3' * 32, 'speech.stop', {'stream': STREAM})
    assert w.outcomes()['1' * 32]['status'] == 'cancelled'
    # Stopped stream stays known: a backchannel may not reuse it.
    w.act('d' * 32, 'speech.backchannel', {'stream': STREAM, 'cue': 'agree'})
    assert w.accepts()['d' * 32]['error']['code'] == 'conflict'
    cue, stop = 'e' * 32, 'f' * 32
    w.act(cue, 'speech.backchannel', {'stream': '9' * 32, 'cue': 'thinking'})
    assert w.accepts()[cue]['accepted'] is True
    w.act(stop, 'speech.stop', {'stream': '9' * 32})
    assert w.outcomes()[cue]['status'] == 'cancelled'
    assert w.outcomes()[stop]['status'] == 'succeeded'
    assert [e['payload']['id'] for e in w.events('action.outcome')].index(cue) < \
        [e['payload']['id'] for e in w.events('action.outcome')].index(stop)
    before = len(w.events('speech.position'))
    w.settle(0.3)
    assert all(e['payload']['item'] != cue for e in w.events('speech.position')[before:])
    # A fresh stream plays after the speaker is idle, and a speak cannot join it.
    done = '7' * 32
    w.act(done, 'speech.backchannel', {'stream': '8' * 32, 'cue': 'agree'})
    w.speak('6' * 32, 0, 'nope', True, stream='4' * 32)
    assert w.accepts()['6' * 32]['error']['code'] == 'conflict'
    w.until(lambda w: done in w.outcomes())
    assert w.outcomes()[done]['status'] == 'succeeded'
    w.speak('5' * 32, 0, 'later', True, stream='4' * 32)
    assert w.accepts()['5' * 32]['accepted'] is True


@pytest.mark.parametrize('kind', ['backchannel', 'speak'])
def test_deadline_expiry_stops_audio_and_fences_playback(wire, kind):
    w = wire()
    aid = 'a' * 32
    if kind == 'backchannel':
        capability, args = 'speech.backchannel', {'stream': 'b' * 32, 'cue': 'thinking'}
    else:
        capability, args = 'speech.speak_chunk', {
            'stream': STREAM, 'chunk': 0, 'text': 'hello there', 'final': True}
    # Admit without ticking the device. Render is queued; expiry must fence it.
    w.surface.receive(encode(WIRE, {'type': 'act', 'body': {'grant_epoch': 1, 'action': {
        'id': aid, 'coglet': COGLET, 'capability': capability, 'args': args,
        'deadline': w.surface.now() + 80_000, 'correlation_id': aid}}}))
    w.messages.extend(decode(WIRE, raw) for raw in w.surface.drain())
    assert w.accepts()[aid]['accepted'] is True
    assert w.pending() == [aid]
    time.sleep(0.1)
    w.surface.tick()
    w.pump()
    assert w.outcomes()[aid]['status'] == 'expired'
    assert w.events('speech.position') == []
    w.settle(0.3)
    assert w.events('speech.position') == []
    assert w.outcomes()[aid]['status'] == 'expired'


def test_chunk_expiry_cancels_later_chunks_and_closes_the_stream(wire):
    w = wire()
    first, second = 'a' * 32, 'b' * 32
    now = w.surface.now()
    for aid, chunk, deadline in ((first, 0, now + 80_000), (second, 1, now + 30_000_000)):
        w.surface.receive(encode(WIRE, {'type': 'act', 'body': {'grant_epoch': 1, 'action': {
            'id': aid, 'coglet': COGLET, 'capability': 'speech.speak_chunk',
            'args': {'stream': STREAM, 'chunk': chunk, 'text': 'x' * 40, 'final': False},
            'deadline': deadline, 'correlation_id': aid}}}))
        w.messages.extend(decode(WIRE, raw) for raw in w.surface.drain())
        assert w.accepts()[aid]['accepted'] is True
    assert sorted(w.pending()) == sorted([first, second])
    time.sleep(0.1)
    w.surface.tick()
    w.pump()
    assert w.outcomes()[first]['status'] == 'expired'
    assert w.outcomes()[second]['status'] == 'cancelled'
    assert w.device.engine.streams[STREAM]['stopped'] is True
    w.speak('c' * 32, 2, 'gap', True)
    assert w.accepts()['c' * 32]['error']['code'] == 'conflict'
    w.settle(0.3)
    assert w.events('speech.position') == []


def test_expression_apply_expiry_fence_and_revocation(wire):
    w = wire()
    w.query('expression.state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32) == {
        'expression': 'neutral', 'intensity': 0, 'remaining_ms': 0}
    w.act('a' * 32, 'expression.set_expression',
          {'expression': 'product_foo', 'intensity': 1, 'duration_ms': 10})
    assert w.accepts()['a' * 32]['error']['code'] == 'invalid_argument'
    w.act('1' * 32, 'expression.set_expression',
          {'expression': 'listening', 'intensity': 500, 'duration_ms': 40})
    assert w.outcomes()['1' * 32]['status'] == 'succeeded'
    assert w.outcomes()['1' * 32]['result'] == {}
    # A newer expression fences the previous timer.
    w.act('2' * 32, 'expression.set_expression',
          {'expression': 'happy', 'intensity': 200, 'duration_ms': 5000})
    time.sleep(0.08)
    w.query('expression.state', {}, 'd' * 32)
    state = ok_value(w.reply(), 'd' * 32)
    assert state['expression'] == 'happy'
    assert state['intensity'] == 200
    assert state['remaining_ms'] > 0
    w.act('3' * 32, 'expression.set_expression',
          {'expression': 'concerned', 'intensity': 9, 'duration_ms': 0})
    w.query('expression.state', {}, 'e' * 32)
    assert ok_value(w.reply(), 'e' * 32) == {
        'expression': 'neutral', 'intensity': 0, 'remaining_ms': 0}
    w.act('4' * 32, 'expression.set_expression',
          {'expression': 'speaking', 'intensity': 10, 'duration_ms': 60000})
    w.surface.grant([])
    w.pump()
    assert w.device.engine.expression == 'neutral'
    epoch = w.surface._get('epoch')
    w.act('5' * 32, 'expression.set_expression',
          {'expression': 'happy', 'intensity': 1, 'duration_ms': 10}, epoch=epoch)
    assert w.accepts()['5' * 32]['error']['code'] == 'permission_denied'
    w.surface.grant(grant_entries(operations_for('')))
    w.pump()
    epoch = w.surface._get('epoch')
    w.query('expression.state', {}, 'f' * 32, epoch=epoch)
    assert ok_value(w.reply(), 'f' * 32)['expression'] == 'neutral'
    killed = wire()
    killed.act('1' * 32, 'expression.set_expression',
               {'expression': 'listening', 'intensity': 1, 'duration_ms': 60000})
    killed.surface.kill('probe')
    killed.pump()
    assert killed.device.engine.expression == 'neutral'
    killed.act('2' * 32, 'expression.set_expression',
               {'expression': 'happy', 'intensity': 1, 'duration_ms': 10},
               epoch=killed.surface._get('epoch'))
    assert killed.accepts()['2' * 32]['error']['code'] == 'permission_denied'
    dropped = wire()
    dropped.act('1' * 32, 'expression.set_expression',
                {'expression': 'thinking', 'intensity': 1, 'duration_ms': 60000})
    dropped.surface.disconnect()
    assert dropped.device.engine.expression == 'neutral'


def test_work_state_read_and_rejection(wire):
    w = wire('duplicate-call')
    w.query('speech.work_state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32) == {'epoch': 1, 'pending': 1}
    w.query('speech.work_state', {'epoch': 1}, 'd' * 32)
    assert w.reply()['error']['code'] == 'invalid_argument'
    w.surface.grant([])
    w.pump()
    w.query('speech.work_state', {}, 'e' * 32, epoch=w.surface._get('epoch'))
    assert w.reply()['error']['code'] == 'permission_denied'


def test_work_intent_dedup_and_start_cancels_without_barge(wire):
    w = wire('duplicate-call')
    assert len(w.events('speech.work_intent')) == 1
    assert w.events('speech.work_intent')[0]['payload']['call'] == CALL
    with pytest.raises(ValueError, match='conflict'):
        w.device.engine.stage_work(0, 'e' * 32, CALL, 'game.inspect', {'request': 'Other'})
    w.pump()
    assert len(w.events('speech.work_intent')) == 1
    from coglet_surface.codec import encode_profile
    from coglet_sim.duplex import WORK_ARGUMENT_BYTES
    huge = {'x': 'y' * 9000}
    assert len(encode_profile(huge)) > WORK_ARGUMENT_BYTES
    assert w.device.engine.stage_work(0, '7' * 32, '8' * 32, 'game.inspect', huge) == []
    w.pump()
    assert len(w.events('speech.work_intent')) == 1
    w.device.emit_engine(w.device.engine.feed(0, 'start', utterance='6' * 32, position_ms=0))
    w.pump()
    assert w.events('speech.barge_in') == []
    assert work_payloads(w) == [{
        'turn': TURN, 'call': CALL, 'epoch': 1, 'status': 'cancelled',
        'value': {'code': 'cancelled', 'message': 'No grant', 'retryable': False},
    }]
    w.query('speech.work_state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32) == {'epoch': 2, 'pending': 0}


def test_work_intent_refuses_a_fifth_live_call(wire):
    w = wire('duplicate-call')
    for index in range(3):
        call = f'{index + 1:032x}'
        w.device.emit_engine(w.device.engine.stage_work(
            0, TURN, call, 'game.inspect', {'request': 'Inspect', 'n': index}))
    w.pump()
    assert len(w.events('speech.work_intent')) == 4
    w.query('speech.work_state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32) == {'epoch': 1, 'pending': 4}
    with pytest.raises(ValueError, match='resource_exhausted'):
        w.device.engine.stage_work(0, TURN, 'f' * 32, 'game.inspect', {'request': 'More'})
    w.pump()
    w.query('speech.work_state', {}, 'd' * 32)
    assert ok_value(w.reply(), 'd' * 32) == {'epoch': 1, 'pending': 4}
    assert len(w.events('speech.work_intent')) == 4


@pytest.mark.parametrize('status, value', [
    ('failed', FAILED),
    ('succeeded', INSPECTED),
])
def test_work_result_preserves_status_and_rejects_duplicates(wire, status, value):
    w = wire('duplicate-call')
    aid = 'a' * 32
    args = {'turn': TURN, 'call': CALL, 'epoch': 1, 'status': status, 'value': value}
    w.act(aid, 'speech.work_result', args)
    assert w.accepts()[aid]['accepted'] is True
    assert work_payloads(w) == [{
        'turn': TURN, 'call': CALL, 'epoch': 1, 'status': status, 'value': value}]
    assert w.outcomes()[aid]['status'] == 'succeeded'
    assert w.outcomes()[aid]['result'] == {}
    order = [e['kind'] for e in w.events() if e['kind'] in ('speech.work_done', 'action.outcome')]
    assert order[:2] == ['speech.work_done', 'action.outcome']
    w.act(aid, 'speech.work_result', args)  # exact retry: historical receipt, no second done
    assert w.accepts()[aid]['accepted'] is True
    assert len(work_payloads(w)) == 1
    w.act('b' * 32, 'speech.work_result', args)
    assert w.accepts()['b' * 32]['error']['code'] == 'not_found'
    assert len(work_payloads(w)) == 1
    w.query('speech.work_state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32)['pending'] == 0


def test_stale_mismatched_and_oversized_work_result(wire):
    w = wire('duplicate-call')
    w.act('c' * 32, 'speech.work_result', {
        'turn': TURN, 'call': CALL, 'epoch': 2, 'status': 'failed', 'value': FAILED})
    assert w.accepts()['c' * 32]['error']['code'] == 'not_found'
    w.act('d' * 32, 'speech.work_result', {
        'turn': 'e' * 32, 'call': CALL, 'epoch': 1, 'status': 'failed', 'value': FAILED})
    assert w.accepts()['d' * 32]['error']['code'] == 'conflict'
    from coglet_surface.codec import encode_profile
    from coglet_sim.duplex import WORK_RESULT_BYTES
    huge = {'x': 'y' * 40000}
    assert len(encode_profile(huge)) > WORK_RESULT_BYTES
    w.act('f' * 32, 'speech.work_result', {
        'turn': TURN, 'call': CALL, 'epoch': 1, 'status': 'succeeded', 'value': huge})
    assert w.accepts()['f' * 32]['error']['code'] == 'invalid_argument'
    assert work_payloads(w) == []
    w.query('speech.work_state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32) == {'epoch': 1, 'pending': 1}
    w.act('a' * 32, 'speech.work_result', {
        'turn': TURN, 'call': CALL, 'epoch': 1, 'status': 'failed', 'value': FAILED})
    assert work_payloads(w)[0]['status'] == 'failed'
    assert len(work_payloads(w)) == 1


def test_work_cancelled_emits_one_done_and_rejects_a_result(wire):
    w = wire('work-cancelled')
    assert work_payloads(w) == [{
        'turn': TURN, 'call': CALL, 'epoch': 1, 'status': 'cancelled',
        'value': {'code': 'cancelled', 'message': 'No grant', 'retryable': False},
    }]
    w.query('speech.work_state', {}, 'c' * 32)
    assert ok_value(w.reply(), 'c' * 32) == {'epoch': 2, 'pending': 0}
    w.act('a' * 32, 'speech.work_result', {
        'turn': TURN, 'call': CALL, 'epoch': 1, 'status': 'succeeded', 'value': INSPECTED})
    assert w.accepts()['a' * 32]['error']['code'] == 'not_found'
    assert len(work_payloads(w)) == 1

