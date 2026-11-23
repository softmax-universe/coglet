"""Replay provenance contracts, not production admission or engine execution."""
import asyncio
import copy
import json
from types import SimpleNamespace
import unittest

import cbor2
from reference_cddl import Schema, ValidationError
import test_abi
from test_abi import check_trace, digest, record_hash
from test_common import SPEC, decode
from test_surface_case_timeline import Writer, harness
from wire import encode


class ProvenanceTrace(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        test_abi.AbiFixtures.setUpClass()
        cls.validators = test_abi.AbiFixtures()
        cls.vectors = json.loads((SPEC / 'fixtures/abi/provenance-cases.fixture').read_text())
        cls.timelines = json.loads((SPEC.parent / 'conformance/abi/provenance.json').read_text())

    async def test_rehashed_provenance_and_timelines(self):
        self.assertEqual(len(self.vectors), len(self.timelines))
        cddl = (SPEC / 'messages/hub.cddl').read_text() + '\n' + (SPEC / 'messages/common.cddl').read_text()
        for vector, case in zip(self.vectors, self.timelines):
            with self.subTest(name=vector['name']):
                trace = vector['value']
                self.assertEqual(case['name'], vector['name'])
                self.assertEqual(case['steps'][0]['send'], {'operation': 'abi.validate_trace', 'trace': trace})
                self.validators.validator('trace').validate(trace)
                Schema('fixture = trace\n' + self.validators.cddl).validate_cbor(cbor2.dumps(trace))
                rec = trace['records'][0]
                self.assertEqual(rec['record_hash'], record_hash(rec))
                for side in ('input', 'output'):
                    self.assertEqual(rec[side + '_hash'], digest(rec[side + '_files']))
                self.assertEqual(trace['initial_hash'], rec['before_hash'])
                try:
                    check_trace(trace, self.validators.validator('manifest'))
                    accepted = True
                except ValueError as error:
                    self.assertIn('provenance', str(error))
                    accepted = False
                self.assertEqual(accepted, vector['valid'])
                # Independently exercise the shared hub rule for decodable records.
                raw_hex = rec['input_files'].get('/code/build/provenance.cbor')
                if raw_hex is not None and vector['name'] not in {
                    'provenance-malformed-cbor', 'provenance-trailing-cbor', 'provenance-noncanonical-cbor'
                }:
                    raw = bytes.fromhex(raw_hex)
                    decode(raw)
                    if vector['name'] in {'provenance-missing-policy', 'provenance-unknown-field'}:
                        with self.assertRaises(ValidationError):
                            Schema('fixture = hub-build-record\n' + cddl).validate_cbor(raw)
                    else:
                        Schema('fixture = hub-build-record\n' + cddl).validate_cbor(raw)
                for corrupt in (False, True):
                    peer = SimpleNamespace(impl={'transport': 'stdio'}, writer=Writer(), reader=asyncio.StreamReader())
                    raw = encode({'valid': not accepted if corrupt else accepted, 'dispatched': False})
                    peer.reader.feed_data(len(raw).to_bytes(4, 'big') + raw)
                    peer.reader.feed_eof()
                    if corrupt:
                        with self.assertRaises(AssertionError):
                            await harness.timeline(peer, copy.deepcopy(case))
                    else:
                        await harness.timeline(peer, copy.deepcopy(case))
                        self.assertEqual(await peer.reader.read(), b'')
                        self.assertTrue(peer.writer.frames)
