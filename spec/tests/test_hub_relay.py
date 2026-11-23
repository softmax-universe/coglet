"""Relay case-contract oracle, not a hub implementation or conformance claim."""
import asyncio
import copy
import json
import unittest
from types import SimpleNamespace

from test_common import SPEC, encode
from test_surface_case_timeline import Writer, harness


class RelayOracle:
    def __init__(self):
        self.positions = {}
        self.receipts = {}
        self.watermarks = {}
        self.forwarded = []

    def admit(self, request):
        target = request['target']
        params = request['params']
        offset, events = params['offset'], params['events']
        position = self.positions.get(target, 0)
        key = (target, offset)
        raw = encode(events)
        def fail(code):
            return {'type': 'error', 'correlation_id': request['correlation_id'],
                    'error': {'code': code, 'message': 'relay refused', 'retryable': False}}
        for event in events:
            source = event['source']
            if event['kind'].startswith('runtime.') or not (source.startswith('surface:') or
                    (source == target and event['kind'] == 'action.outcome')):
                return fail('permission_denied')
        if events and key in self.receipts:
            recorded, result = self.receipts[key]
            if recorded != raw:
                return fail('conflict')
        else:
            if offset != position:
                return fail('conflict')
            if offset + len(events) > 2**64 - 1:
                return fail('resource_exhausted')
            pending = dict(self.watermarks)
            for event in events:
                source = (target, event['source'])
                if event['seq'] <= pending.get(source, -1):
                    return fail('conflict')
                pending[source] = event['seq']
            result = position + len(events)
            self.watermarks = pending
            self.positions[target] = result
            if events:
                self.receipts[key] = (raw, result)
                self.forwarded.extend(copy.deepcopy(events))
        return {'type': 'reply', 'correlation_id': request['correlation_id'],
                'result': {'next_offset': result}}


class RelayContracts(unittest.IsolatedAsyncioTestCase):
    async def test_event_case_timelines_and_negative_controls(self):
        for path in sorted((SPEC.parent / 'conformance/hub').glob('event-*.json')):
            case = json.loads(path.read_text())
            oracle = RelayOracle()
            frames = [oracle.admit(s['send']) for s in case['steps'] if 'send' in s]
            for corrupt in [False, True]:
                peer = SimpleNamespace(impl={'transport': 'stdio'}, writer=Writer(), reader=asyncio.StreamReader())
                for index, frame in enumerate(frames):
                    value = copy.deepcopy(frame)
                    if corrupt and index == len(frames) - 1:
                        if value['type'] == 'reply':
                            value['result']['next_offset'] += 1
                        else:
                            value['error']['code'] = 'internal'
                    raw = encode(value)
                    peer.reader.feed_data(len(raw).to_bytes(4, 'big') + raw)
                peer.reader.feed_eof()
                with self.subTest(case=case['name'], corrupt=corrupt):
                    if corrupt:
                        with self.assertRaises(AssertionError):
                            await harness.timeline(peer, copy.deepcopy(case))
                    else:
                        await harness.timeline(peer, copy.deepcopy(case))
                        self.assertEqual(await peer.reader.read(), b'')
            if path.stem == 'event-batch-dedup':
                self.assertEqual(oracle.forwarded, case['steps'][0]['send']['params']['events'])

    def test_payload_bytes_preserved(self):
        case = json.loads((SPEC.parent / 'conformance/hub/event-batch-dedup.json').read_text())
        request = case['steps'][0]['send']
        event = request['params']['events'][0]
        speech = json.loads((SPEC / 'fixtures/speech/cases.json').read_text())
        payload = next(c['value'] for c in speech if c['type'] == 'speech-end' and c['valid'])
        event.update(kind='speech.end', payload=payload)
        before = encode(event)
        oracle = RelayOracle()
        self.assertEqual(oracle.admit(request)['type'], 'reply')
        self.assertEqual(encode(oracle.forwarded[0]), before)
        self.assertNotIn('provenance', oracle.forwarded[0]['payload'])
