"""Actual CDDL/JSON Schema equivalence for the sandbox golden corpus."""
import json
from pathlib import Path
import unittest

from jsonschema import Draft202012Validator
from reference_cddl import Schema, ValidationError
from test_common import encode
import build_sandbox

SPEC = Path(__file__).resolve().parents[1]


class SandboxContracts(unittest.TestCase):
    def test_sandbox_golden_both_schemas(self):
        schema = json.loads((SPEC/'messages/sandbox.schema.json').read_text())
        cddl = (SPEC/'messages/sandbox.cddl').read_text()
        fixtures = json.loads((SPEC/'fixtures/sandbox/cases.json').read_text())
        for fixture in fixtures:
            with self.subTest(case=fixture['name']):
                selected = fixture['type']
                validator = Draft202012Validator({**schema, '$ref':'#/$defs/'+selected})
                self.assertEqual(validator.is_valid(fixture['value']), fixture['valid'])
                oracle = Schema('selected = '+selected+'\n'+cddl)
                try:
                    oracle.validate_cbor(encode(fixture['value']))
                    actual = True
                except ValidationError:
                    actual = False
                self.assertEqual(actual, fixture['valid'])
                if actual:
                    self.assertEqual(encode(fixture['value']).hex(), fixture['cbor_hex'])
        print(f'Sandbox: {len(fixtures)} fixtures checked with both schemas')

    def test_sandbox_sources_current(self):
        for path, expected in build_sandbox.outputs().items():
            self.assertEqual((SPEC.parent/path).read_text(), expected, path)


if __name__ == '__main__':
    unittest.main()
