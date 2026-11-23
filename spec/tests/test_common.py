"""Schema fixtures, not a protocol implementation conformance claim."""
import io
import json
from pathlib import Path
import unittest

import cbor2
from jsonschema import Draft202012Validator
from pycddl import Schema, ValidationError

SPEC = Path(__file__).resolve().parents[1]


def encode(value):
    """Core deterministic order, deliberately NOT cbor2 canonical=True."""
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value):
            raise ValueError('map keys must be text')
        pairs = sorted((encode(k), encode(v)) for k, v in value.items())
        return header(5, len(pairs)) + b''.join(k + v for k, v in pairs)
    if isinstance(value, list):
        return header(4, len(value)) + b''.join(map(encode, value))
    if value is None or isinstance(value, (bool, str)):
        return cbor2.dumps(value)
    if type(value) is int and -(2**63) <= value < 2**64:
        return cbor2.dumps(value)
    raise ValueError('value outside common CBOR profile')


def header(major, size):
    raw = cbor2.dumps(size)
    return bytes([(major << 5) | (raw[0] & 31)]) + raw[1:]


def decode(raw):
    stream = io.BytesIO(raw)
    value = cbor2.CBORDecoder(stream).decode()
    if stream.read() or encode(value) != raw:
        raise ValueError('not a single core deterministic profile item')
    return value


class CommonFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cddl = (SPEC / 'messages/common.cddl').read_text()
        cls.json_schema = json.loads((SPEC / 'messages/common.schema.json').read_text())
        Draft202012Validator.check_schema(cls.json_schema)
        cls.cases = json.loads((SPEC / 'fixtures/common/cases.json').read_text())

    def test_all_fixtures_against_both_schemas(self):
        self.assertTrue(self.cases)
        self.assertEqual(len(self.cases), len({c['name'] for c in self.cases}))
        valid_count = 0
        for case in self.cases:
            with self.subTest(case=case['name']):
                rule = case['type']
                schema = dict(self.json_schema, **{'$ref': f'#/$defs/{rule}'})
                schema.pop('anyOf', None)
                js_valid = Draft202012Validator(schema).is_valid(case['value'])
                try:
                    Schema(f'fixture = {rule}\n' + self.cddl).validate_cbor(
                        cbor2.dumps(case['value']))
                    cddl_valid = True
                except ValidationError:
                    cddl_valid = False
                self.assertEqual(js_valid, case['valid'], 'JSON Schema')
                self.assertEqual(cddl_valid, case['valid'], 'CDDL')
                if case['valid']:
                    valid_count += 1
                    raw = bytes.fromhex(case['cbor_hex'])
                    self.assertEqual(decode(raw), case['value'])
                    self.assertEqual(encode(case['value']), raw)
                    self.assertEqual(encode(decode(raw)), raw)
                    # Independent encoder oracle: core and length-first coincide
                    # for this profile's text-only map keys.
                    self.assertEqual(cbor2.dumps(case['value'], canonical=True), raw)
                    Schema(f'fixture = {rule}\n' + self.cddl).validate_cbor(raw)
        print(f'common: {len(self.cases)} dual-schema fixtures; '
              f'{valid_count} valid golden round-trips')

    def test_reject_non_profile_wire(self):
        cases = json.loads((SPEC / 'fixtures/common/wire-invalid.json').read_text())
        self.assertTrue(cases)
        for case in cases:
            with self.subTest(case=case['name']):
                with self.assertRaises((ValueError, cbor2.CBORDecodeError, UnicodeError)):
                    decode(bytes.fromhex(case['cbor_hex']))
        print(f'common: {len(cases)} rejected wire fixtures')

    def test_core_order_text_keys(self):
        # RFC 8949 core sorts encoded keys: 0x617a before 0x626161.
        self.assertEqual(encode({'aa': 1, 'z': 2}).hex(), 'a2617a0262616101')
        # With text-only keys, shortest heads imply length-first order as well.
        # This profile's key restriction makes both orders coincide.
        self.assertEqual(encode({'aa': 1, 'z': 2}),
                         cbor2.dumps({'aa': 1, 'z': 2}, canonical=True))


if __name__ == '__main__':
    unittest.main()
