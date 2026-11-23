"""M-A archive and real-harness case contracts, NOT engine conformance."""
import asyncio
import copy
import json
from types import SimpleNamespace
import unittest

import cbor2
from reference_cddl import Schema
import test_abi
from test_abi import check_trace, digest, record_hash
from test_common import SPEC, encode, decode
from test_surface_case_timeline import Writer, harness
from wire import encode as wire_encode
from build_boundary_receipts import generate


class BoundaryReceipts(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        test_abi.AbiFixtures.setUpClass()
        cls.validators = test_abi.AbiFixtures()
        cls.vectors = json.loads((SPEC / 'fixtures/abi/boundary-receipts.fixture').read_text())
        cls.timelines = json.loads((SPEC.parent / 'conformance/abi/boundary-receipts.json').read_text())

    def test_generator_and_named_coverage(self):
        self.assertEqual((self.vectors, self.timelines), generate(write_files=False))
        names = {v['name'] for v in self.vectors}
        self.assertEqual(len(names), len(self.vectors))
        self.assertTrue({'standalone-durable-write', 'trailing-durable-write',
                         'terminal-guest-rejection-standalone', 'terminal-guest-outcome-standalone',
                         'terminal-rejection-wake-delivery', 'terminal-outcome-wake-delivery',
                         'terminal-after-aborted-emit'} <= names)

    async def test_rehashed_boundary_and_terminal_contracts(self):
        validator = self.validators.validator('trace')
        schema = Schema('fixture = trace\n' + self.validators.cddl)
        for vector, case in zip(self.vectors, self.timelines, strict=True):
            with self.subTest(name=vector['name']):
                trace = vector['value']
                self.assertEqual(case['name'], vector['name'])
                self.assertEqual(case['steps'][0]['send'], dict(operation='abi.validate_trace', trace=trace))
                validator.validate(trace)
                schema.validate_cbor(cbor2.dumps(trace))
                self.assertEqual(trace['initial_hash'], digest(trace['initial_files']))
                for record in trace['records'] + ([trace['trailing_boundary']] if trace['trailing_boundary'] else []):
                    self.assertEqual(record_hash(record), record['record_hash'])
                    self.assertEqual(digest(record['output_files']), record['output_hash'])
                    raw = encode(record)
                    frame = len(raw).to_bytes(4, 'big') + raw
                    self.assertEqual(decode(frame[4:]), record)
                if vector['valid']:
                    state = check_trace(trace, self.validators.validator('manifest'))
                    self.assertEqual(state, vector['state'])
                    # An independent projection of terminal pending IDs. The oracle
                    # above also checks boundaries, hashes, permissions and ordering.
                    pending = {a['id'] for a in trace['initial_actions']}
                    for rec in trace['records']:
                        # Input terminal lines consume earlier commits, never this
                        # attempt's emits. Initial lines are already accounted for.
                        for change in rec['boundary']:
                            self._consume_terminal_ids(change, pending)
                        pending.update(a['id'] for a in rec['actions'])
                    if trace['trailing_boundary']:
                        for change in trace['trailing_boundary']['boundary']:
                            self._consume_terminal_ids(change, pending)
                    self.assertEqual(sorted(pending), state['pending_actions'])
                    result = dict(valid=True, dispatched=False, engine_invocations=0, state=state)
                else:
                    with self.assertRaisesRegex(ValueError, vector['error']):
                        check_trace(trace, self.validators.validator('manifest'))
                    result = dict(valid=False, dispatched=False, engine_invocations=0)
                # Exercise the actual harness with independently calculated frames,
                # then prove both acceptance and no-dispatch assertions are enforced.
                for corrupt in (None, 'valid', 'dispatched', 'engine_invocations'):
                    response = copy.deepcopy(result)
                    if corrupt:
                        response[corrupt] = not response[corrupt] if corrupt != 'engine_invocations' else 1
                    peer = SimpleNamespace(impl={'transport': 'stdio'}, writer=Writer(), reader=asyncio.StreamReader())
                    raw = wire_encode(response)
                    peer.reader.feed_data(len(raw).to_bytes(4, 'big')+raw)
                    peer.reader.feed_eof()
                    if corrupt:
                        with self.assertRaises(AssertionError):
                            await harness.timeline(peer, copy.deepcopy(case))
                    else:
                        await harness.timeline(peer, copy.deepcopy(case))
                        self.assertEqual(await peer.reader.read(), b'')
                        self.assertTrue(peer.writer.frames)

    @staticmethod
    def _consume_terminal_ids(change, pending):
        if change['path'] != '/data/inbox.jsonl':
            return
        old = bytes.fromhex(change['before'] or '')
        new = bytes.fromhex(change['after'] or '')
        for raw in new[len(old):].splitlines():
            event = json.loads(raw)['event']
            if event['kind'] in ('runtime.action_rejected', 'action.outcome'):
                pending.remove(event['payload']['id'])

    def test_zero_step_header_and_closed_boundary_shapes(self):
        sample = copy.deepcopy(next(v['value'] for v in self.vectors if v['name'] == 'standalone-durable-write'))
        self.assertEqual(sample['records'], [])
        rec = sample['trailing_boundary']
        self.assertTrue(self.validators.validator('boundary-record').is_valid(rec))
        for key in ('step', 'calls', 'events', 'engine', 'usage', 'status'):
            bad = dict(rec, **{key: []})
            self.assertFalse(self.validators.validator('boundary-record').is_valid(bad))
        for key in ('initial_files', 'final_hash', 'initial_actions', 'code_hash', 'manifest_hash', 'trailing_boundary'):
            bad = dict(sample)
            del bad[key]
            self.assertFalse(self.validators.validator('trace').is_valid(bad))
        for key in ('code_hash', 'manifest_hash'):
            bad = copy.deepcopy(sample)
            bad[key] = '0'*64
            with self.assertRaises((KeyError, ValueError)):
                check_trace(bad)


if __name__ == '__main__':
    unittest.main()
