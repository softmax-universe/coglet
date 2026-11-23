"""Execute case contracts through the real harness; not device conformance."""
import asyncio
import copy
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
import harness
from wire import encode


class Writer:
    def __init__(self):
        self.frames = []

    def write(self, value):
        self.frames.append(value)

    async def drain(self):
        pass


class KillTimeline(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.case = json.loads((ROOT / 'conformance/surface/kill-latched.json').read_text())
        cid = '0' * 32
        self.frames = [
            self.case['steps'][0]['expect'],
            {'type': 'capabilities', 'body': {'revision': 1}},
            {'type': 'grants', 'body': {'epoch': 1}},
            {'type': 'kill', 'body': {'reason': 'operator', 'correlation_id': cid}},
            {'type': 'health', 'body': {'state': 'stopped', 'clock': 100000,
                                       'pending': 0, 'correlation_id': cid}},
        ]
        action = next(s['send']['body']['action'] for s in self.case['steps']
                      if s.get('send', {}).get('type') == 'act')
        self.frames.append({'type': 'accept', 'body': {
            'id': action['id'], 'correlation_id': action['correlation_id'],
            'accepted': False, 'error': {'code': 'permission_denied',
                                        'message': 'Kill latched', 'retryable': False}}})

    async def run_frames(self, frames, case=None):
        peer = SimpleNamespace()
        peer.impl = {'transport': 'stdio'}
        peer.writer = Writer()
        peer.reader = asyncio.StreamReader()
        for value in frames:
            raw = encode(value)
            peer.reader.feed_data(len(raw).to_bytes(4, 'big') + raw)
        peer.reader.feed_eof()
        await harness.timeline(peer, case or self.case)
        self.assertEqual(await peer.reader.read(), b'', 'unconsumed notification')
        self.assertTrue(peer.writer.frames)

    async def test_kill_stopped_health_then_latched_rejection(self):
        await self.run_frames(self.frames)

    async def test_omitting_health_expectation_reproduces_review_failure(self):
        case = copy.deepcopy(self.case)
        case['steps'] = [s for s in case['steps']
                         if s.get('match', {}).get('type') != 'health']
        with self.assertRaisesRegex(AssertionError, "predicate type != 'accept'"):
            await self.run_frames(self.frames, case)

    async def test_non_stopped_health_fails(self):
        frames = copy.deepcopy(self.frames)
        frames[4]['body']['state'] = 'ready'
        with self.assertRaisesRegex(AssertionError, "body.state != 'stopped'"):
            await self.run_frames(frames)

    async def test_kill_does_not_clear_latch(self):
        frames = copy.deepcopy(self.frames)
        frames[-1]['body']['accepted'] = True
        del frames[-1]['body']['error']
        with self.assertRaisesRegex(AssertionError, 'body.accepted != False'):
            await self.run_frames(frames)
