"""Simulator media drives the real compiled Swift Face adapter, not a response shim.

Run with the sim project's locked Python environment and FACE_MAC_EXECUTABLE.
No native audio, provider, Rust guest or runtime-WAL qualification is implied.
"""
import json
import os
from pathlib import Path
import selectors
import subprocess

import pytest
from coglet_surface.codec import decode, encode
from coglet_surface.surface import WIRE

ROOT = Path(__file__).resolve().parents[2]
TIMELINES = json.loads((ROOT / 'spec/fixtures/speech/timelines.json').read_text())
OWNER = 'coglet:' + 'b' * 64
# The macOS Swift lane explicitly builds and selects this adapter. Ordinary sim
# tests do not provision Swift; that lane's skip is not Face qualification.
pytestmark = pytest.mark.skipif(not os.environ.get('FACE_MAC_EXECUTABLE'),
                                reason='compiled Face adapter not selected')


class Face:
    def __init__(self):
        executable = os.environ.get('FACE_MAC_EXECUTABLE')
        if not executable:
            pytest.fail('FACE_MAC_EXECUTABLE must name the compiled face-mac binary')
        self.process = subprocess.Popen([executable, '--simulator'], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, text=True)
        self.messages = []
        hello = self.command({})['messages'][0]
        self.send('hello_reply', {'selection': {'version': '0.1'},
                                 'correlation_id': hello['body']['correlation_id']})
        self.command({'activate': True})
        self.messages.clear()

    def close(self):
        self.process.stdin.close()
        try:
            code = self.process.wait(timeout=5)
            assert code == 0
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            self.process.stdout.close()

    def command(self, command):
        self.process.stdin.write(json.dumps(command) + '\n')
        self.process.stdin.flush()
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            assert selector.select(5), 'compiled Face adapter failed to respond'
        line = self.process.stdout.readline()
        assert line, f'Face executable exited {self.process.poll()}'
        response = json.loads(line)
        response['messages'] = [decode(WIRE, bytes(frame)) for frame in response['frames']]
        self.messages.extend(response['messages'])
        return response

    def send(self, kind, body):
        return self.command({'wire': list(encode(WIRE, {'type': kind, 'body': body}))})

    def speak(self, aid='1' * 32, chunk=0, text='Hello world', final=True):
        return self.send('act', {'grant_epoch': 1, 'action': {
            'id': aid, 'coglet': OWNER, 'capability': 'speech.speak_chunk',
            'args': {'stream': '2' * 32, 'chunk': chunk, 'text': text, 'final': final},
            'deadline': 30_000_000, 'correlation_id': aid}})

    def events(self):
        return [m['body'] for m in self.messages if m['type'] == 'event']


@pytest.fixture
def face():
    driver = Face()
    try:
        yield driver
    finally:
        driver.close()


@pytest.mark.parametrize('name', ['barge-in-stop', 'late-partial-and-stale-revision'])
def test_compiled_face_conformance_timeline(face, name):
    timeline = next(t for t in TIMELINES if t['name'] == name)
    if name == 'barge-in-stop':
        face.command({'synthesis': {'duration_ms': 3000}})
    previous = 0
    for step in timeline['steps']:
        kind = step['input']
        command = {'now': step['at_ms'] * 1000}
        if name == 'barge-in-stop':
            command['render'] = (step['at_ms'] - previous) * 24
        previous = step['at_ms']
        if kind == 'play':
            face.speak()
            continue
        if kind == 'barge_in':
            continue  # Real native start/reflex already emitted it; never inject it.
        if kind == 'start':
            command.update(start=True, position_ms=step['position_ms'])
        elif kind in ('partial', 'end'):
            command['recognition'] = dict(revision=step['revision'], text=step['text'],
                final=kind == 'end', position_ms=step.get('position_ms', 0))
        response = face.command(command)
    events = face.events()
    assert [e['kind'] for e in events] == timeline['expected']['events']
    assert response['discarded'] == timeline['expected']['discarded']
    if name == 'barge-in-stop':
        barge = next(e['payload'] for e in events if e['kind'] == 'speech.barge_in')
        assert barge['played_ms'] == 100
        assert barge['item'] == '1' * 32
        assert response['queued'] == 0
        outcome = next(e['payload'] for e in events if e['kind'] == 'action.outcome')
        assert outcome['status'] == 'cancelled'
    expected_final = 'wait please' if name == 'barge-in-stop' else 'hello world'
    assert events[-1]['payload']['text'] == expected_final


def test_compiled_face_rendered_completion_not_synthesis_and_stop_queue(face):
    face.command({'synthesis': {'duration_ms': 100, 'spans': [{'end_ms': 100, 'text': 'hello'}]}})
    face.speak(text='hello', final=False)
    face.speak(aid='3' * 32, chunk=1, text='later', final=True)
    assert not any(e['kind'] == 'action.outcome' for e in face.events())
    face.command({'render': 2400})
    # Only first chunk is rendered; second is merely enqueued on another tick.
    outcomes = [e['payload'] for e in face.events() if e['kind'] == 'action.outcome']
    assert [(o['id'], o['status']) for o in outcomes] == [('1' * 32, 'succeeded')]
    face.send('act', {'grant_epoch': 1, 'action': {'id': '4' * 32, 'coglet': OWNER,
        'capability': 'speech.stop', 'args': {'stream': '2' * 32},
        'deadline': 30_000_000, 'correlation_id': '4' * 32}})
    outcomes = {e['payload']['id']: e['payload']['status'] for e in face.events() if e['kind'] == 'action.outcome'}
    assert outcomes == {'1' * 32: 'succeeded', '3' * 32: 'cancelled', '4' * 32: 'succeeded'}
    assert face.command({'render': 24000})['rendered'] == 2400
