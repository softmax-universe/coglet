import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]


class CodecTests(unittest.TestCase):
    def test_fixtures(self):
        self.assertIsNotNone(importlib.util.find_spec('coglet_surface.codec'), 'missing codec')
        from coglet_surface.codec import decode, encode, decode_profile
        valid = invalid = wire = 0
        for path in sorted((ROOT / 'spec/fixtures').rglob('*.json')):
            cases = json.loads(path.read_text())
            self.assertIsInstance(cases, list, str(path))
            for case in cases:
                with self.subTest(file=str(path), case=case['name']):
                    if 'valid' not in case:
                        with self.assertRaises(ValueError):
                            decode_profile(bytes.fromhex(case['cbor_hex']))
                        wire += 1
                    elif case['valid']:
                        raw = bytes.fromhex(case['cbor_hex'])
                        self.assertEqual(encode(case['type'], decode(case['type'], raw)), raw)
                        self.assertEqual(encode(case['type'], case['value']), raw)
                        valid += 1
                    else:
                        with self.assertRaises(ValueError):
                            encode(case['type'], case['value'])
                        import cbor2
                        with self.assertRaises(ValueError):
                            decode(case['type'], cbor2.dumps(case['value']))
                        invalid += 1
        self.assertGreater(valid, 0)
        print(f'Python fixtures: {valid} golden, {invalid} semantic-invalid, {wire} wire-invalid')

    def test_generated_dataclasses(self):
        from coglet_surface import generated as g
        cases = json.loads((ROOT / 'spec/fixtures/common/cases.json').read_text())
        count = 0
        for case in cases:
            if not case['valid'] or case['type'] not in g.RECORDS:
                continue
            raw = bytes.fromhex(case['cbor_hex'])
            for cls in g.RECORDS[case['type']]:
                try:
                    obj = cls.from_cbor(raw)
                except (TypeError, ValueError, KeyError):
                    continue
                self.assertEqual(obj.to_cbor(), raw)
                count += 1
                break
            else:
                self.fail('no generated dataclass decoded ' + case['name'])
        self.assertEqual(count, sum(c['valid'] and c['type'] in g.RECORDS for c in cases))
        self.assertGreater(count, 0)

    def test_profile_guards(self):
        from coglet_surface.codec import decode_profile, encode_profile
        for raw in ('d9d9f700', 'f814', 'f815', 'f816', 'a16161d9d9f700',
                    '3b8000000000000000', '9bffffffffffffffff', '81' * 66 + '00'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                decode_profile(bytes.fromhex(raw))
        for value in (b'a', 1.0, {1: 'x'}, 2**64, -(2**63)-1, float('nan')):
            with self.subTest(value=value), self.assertRaises(ValueError):
                encode_profile(value)


if __name__ == '__main__':
    unittest.main()
