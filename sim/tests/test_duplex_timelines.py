"""Named tests: every conformance speech timeline through DuplexEngine."""
import json
from pathlib import Path

import pytest

from coglet_sim.duplex import DuplexEngine, heard_prefix

ROOT = Path(__file__).resolve().parents[2]
TIMELINES = json.loads((ROOT / 'spec/fixtures/speech/timelines.json').read_text())
CASES = sorted(p.stem for p in (ROOT / 'conformance/speech').glob('*.json'))


@pytest.mark.parametrize('timeline', TIMELINES, ids=lambda t: t['name'])
def test_timeline_heard_played_truncation(timeline):
    engine = DuplexEngine()
    result = engine.run_timeline(timeline)
    expected = timeline['expected']
    assert result['events'] == expected['events']
    assert result['discarded'] == expected['discarded']
    assert result['output'] == expected['output']
    assert result['terminal'] == expected['terminal']


@pytest.mark.parametrize('timeline', TIMELINES, ids=lambda t: t['name'])
def test_timeline_replay_equality(timeline):
    first = DuplexEngine()
    second = DuplexEngine()
    a = first.run_timeline(timeline)
    b = second.run_timeline(timeline)
    assert a == b
    restored = DuplexEngine()
    restored.restore(first.snapshot())
    assert restored.snapshot()['journal'] == first.snapshot()['journal']
    assert restored.events == first.events


def test_heard_truncation_hello_world_1200_of_3000():
    engine = DuplexEngine(barge='stop', render_latency_ms=0)
    engine.spans = [{'end_ms': 1000, 'text': 'Hello'}, {'end_ms': 3000, 'text': ' world'}]
    engine.feed(0, 'play', stream='2' * 32, chunk=0, item='1' * 32, played_ms=0)
    engine.feed(0, 'start', utterance='4' * 32, position_ms=0)
    engine.feed(1200, 'barge_in', utterance='4' * 32, stream='2' * 32, item='1' * 32,
                played_ms=1200, position_ms=1200)
    record = engine.heard('1' * 32)
    assert record == {'item': '1' * 32, 'played_ms': 1200, 'text': 'Hello'}
    assert heard_prefix(engine.spans, 999) == ''
    assert heard_prefix(engine.spans, 1000) == 'Hello'
    assert heard_prefix(engine.spans, 3000) == 'Hello world'


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
