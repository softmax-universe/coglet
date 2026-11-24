"""Named tests: every frozen speech timeline through DuplexEngine.

Asserts event order, played/heard/truncation results, and replay equality: an
independent re-execution, and continuation from every intermediate snapshot
(after a serialization round trip), both reproduce the uninterrupted run.
"""
import json
from pathlib import Path

import pytest

from coglet_sim.duplex import DuplexEngine, heard_prefix

ROOT = Path(__file__).resolve().parents[2]
TIMELINES = json.loads((ROOT / 'spec/fixtures/speech/timelines.json').read_text())
CASES = sorted(p.stem for p in (ROOT / 'conformance/speech').glob('*.json'))
ITEM, STREAM, UTTERANCE = '1' * 32, '2' * 32, '4' * 32
HELLO_WORLD = [{'end_ms': 1000, 'text': 'Hello'}, {'end_ms': 3000, 'text': ' world'}]


@pytest.mark.parametrize('timeline', TIMELINES, ids=lambda t: t['name'])
def test_timeline_heard_played_truncation(timeline):
    engine = DuplexEngine()
    result = engine.run_timeline(timeline)
    expected = timeline['expected']
    assert result['events'] == expected['events']
    assert result['discarded'] == expected['discarded']
    assert result['output'] == expected['output']
    assert result['terminal'] == expected['terminal']
    barge = [e['payload'] for e in result['journal'] if e['name'] == 'speech.barge_in']
    step = next((s for s in timeline['steps'] if s['input'] == 'barge_in'), None)
    if step:
        # Barge-in carries the scripted actual output position; the item's
        # played_ms is that cutoff (or the later finish position under duck).
        assert barge[0]['played_ms'] == step['played_ms']
        heard = engine.heard(step['item'])
        finish = next((s for s in timeline['steps'] if s['input'] == 'finish'), None)
        assert heard['played_ms'] == (finish['played_ms'] if finish else step['played_ms'])
        assert heard['text'] == ''  # timelines carry no alignment: unknown -> empty
    else:
        assert barge == []


@pytest.mark.parametrize('timeline', TIMELINES, ids=lambda t: t['name'])
def test_timeline_replay_equality(timeline):
    reference = DuplexEngine()
    final = reference.run_timeline(timeline)
    expected = reference.snapshot()
    # Independent re-execution from the same inputs gives identical state.
    again = DuplexEngine()
    assert again.run_timeline(timeline) == final
    assert again.snapshot() == expected
    # Continuation from every intermediate snapshot (not just a final copy).
    for cut in range(len(timeline['steps']) + 1):
        prefix = DuplexEngine()
        prefix.run_timeline(timeline, upto=cut)
        resumed = DuplexEngine(barge='duck' if timeline['config']['barge'] == 'stop' else 'stop')
        resumed.restore(json.loads(json.dumps(prefix.snapshot())))  # survives serialization
        assert resumed.continue_timeline(timeline, cut) == final, cut
        assert resumed.snapshot() == expected, cut


def _history_engine():
    """Finished aligned item, finished unaligned item, interrupted item, queued item."""
    engine = DuplexEngine(barge='stop', render_latency_ms=0)
    items = ['a' * 32, 'b' * 32, 'c' * 32, 'd' * 32]
    engine.enqueue(STREAM, 0, items[0], HELLO_WORLD, 3000)
    engine.enqueue(STREAM, 1, items[1], [], 800)
    engine.enqueue(STREAM, 2, items[2], HELLO_WORLD, 3000)
    engine.enqueue(STREAM, 3, items[3], HELLO_WORLD, 3000, final=True)
    engine.render(0, items[0])
    engine.complete_item(3000, items[0])
    engine.render(3000, items[1])
    engine.complete_item(3800, items[1])
    engine.render(3800, items[2])
    engine.feed(5000, 'start', utterance=UTTERANCE, position_ms=0)
    events = engine.feed(5000, 'barge_in', utterance=UTTERANCE, stream=STREAM, item=items[2],
                         played_ms=1200)
    return engine, items, events


def test_heard_history_preserves_items_and_truncates_only_the_interrupted_one():
    engine, items, events = _history_engine()
    assert [e['name'] for e in events] == [
        'speech.barge_in', 'speech.position', 'action.outcome', 'action.outcome']
    assert events[0]['payload'] == {
        'utterance': UTTERANCE, 'stream': STREAM, 'item': items[2], 'played_ms': 1200,
        'position_ms': 3800 + 1200, 'reflex': 'stopped'}
    assert [(e['payload']['id'], e['payload']['status']) for e in events[2:]] == [
        (items[2], 'cancelled'), (items[3], 'cancelled')]
    assert engine.heard(items[0]) == {'item': items[0], 'played_ms': 3000, 'text': 'Hello world'}
    assert engine.heard(items[1]) == {'item': items[1], 'played_ms': 800, 'text': ''}
    assert engine.heard(items[2]) == {'item': items[2], 'played_ms': 1200, 'text': 'Hello'}
    assert engine.heard(items[3]) == {'item': items[3], 'played_ms': 0, 'text': ''}
    assert engine.heard('e' * 32) is None
    # A stopped item never resumes or extends; the stream is closed to new chunks.
    assert engine.complete_item(9000, items[2]) == []
    assert engine.heard(items[2])['played_ms'] == 1200
    with pytest.raises(ValueError):
        engine.enqueue(STREAM, 4, 'f' * 32, [], 100)
    with pytest.raises(ValueError):
        engine.feed(9000, 'barge_in', utterance=UTTERANCE, stream=STREAM, item=items[2], played_ms=1300)


def test_forget_heard_removes_records_but_keeps_tombstones_across_replay():
    engine, items, _ = _history_engine()
    states = {i: engine.items[i]['state'] for i in items}
    engine.forget_heard(items[:3])
    assert [engine.heard(i) for i in items[:3]] == [None, None, None]
    assert engine.heard(items[3]) == {'item': items[3], 'played_ms': 0, 'text': ''}
    assert {i: engine.items[i]['state'] for i in items} == states  # tombstones stay
    with pytest.raises(ValueError):
        engine.enqueue(STREAM, 4, items[0], [], 100)  # identity still known, stream closed
    replay = DuplexEngine()
    replay.restore(engine.snapshot())
    assert replay.snapshot() == engine.snapshot()
    assert replay.heard(items[0]) is None


def test_stop_cancels_queued_and_playing_but_leaves_finished_items():
    engine = DuplexEngine(barge='stop', render_latency_ms=0)
    engine.enqueue(STREAM, 0, 'a' * 32, [], 100)
    engine.enqueue(STREAM, 1, 'b' * 32, [], 100)
    engine.enqueue(STREAM, 2, 'c' * 32, [], 100)
    engine.render(0, 'a' * 32)
    engine.complete_item(100, 'a' * 32)
    engine.render(100, 'b' * 32)
    events = engine.stop_stream(150, STREAM)
    assert [(e['name'], e['payload'].get('id') or e['payload'].get('state')) for e in events] == [
        ('speech.position', 'stopped'), ('action.outcome', 'b' * 32), ('action.outcome', 'c' * 32)]
    assert {e['payload']['status'] for e in events[1:]} == {'cancelled'}
    assert engine.items['a' * 32]['state'] == 'finished'
    assert engine.stop_stream(160, STREAM) == []  # idempotent for a known stopped stream
    assert engine.render(170, 'c' * 32) == []  # cancelled queued work never plays


def test_heard_truncation_hello_world_1200_of_3000():
    engine = DuplexEngine(barge='stop', render_latency_ms=0)
    engine.feed(0, 'play', stream=STREAM, chunk=0, item=ITEM, played_ms=0, spans=HELLO_WORLD,
                duration_ms=3000)
    engine.feed(0, 'start', utterance=UTTERANCE, position_ms=0)
    engine.feed(1200, 'barge_in', utterance=UTTERANCE, stream=STREAM, item=ITEM,
                played_ms=1200, position_ms=1200)
    assert engine.heard(ITEM) == {'item': ITEM, 'played_ms': 1200, 'text': 'Hello'}
    assert heard_prefix(HELLO_WORLD, 999) == ''
    assert heard_prefix(HELLO_WORLD, 1000) == 'Hello'
    assert heard_prefix(HELLO_WORLD, 3000) == 'Hello world'
    assert heard_prefix([{'end_ms': 1, 'text': 'é' * 9000}], 1) == 'é' * 8192


def test_expression_expires_on_virtual_clock():
    engine = DuplexEngine()
    engine.set_expression(0, 'listening', 500, 40)
    assert engine.expression_state(10)['expression'] == 'listening'
    assert engine.expression_state(40)['expression'] == 'neutral'
    engine.set_expression(50, 'happy', 1, 0)
    assert engine.expression_state(50) == {'expression': 'neutral', 'intensity': 0, 'remaining_ms': 0}


def test_every_conformance_speech_case_is_named():
    assert CASES == [
        'barge-in-stop', 'duplicate-call', 'heard-truncation',
        'late-partial-and-stale-revision', 'mode-hands-free', 'mode-push-to-talk',
        'over-talk-duck', 'work-cancelled',
    ]
    timeline_names = {t['name'] for t in TIMELINES}
    assert timeline_names <= set(CASES)
