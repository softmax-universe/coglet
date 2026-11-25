"""ADR-0030 schema proof only; no claim of live 0.2 session support."""
import json
from pathlib import Path
import subprocess
import sys
import unittest

import cbor2
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

from reference_cddl import Schema, ValidationError
from test_common import decode, encode

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'sdk/surface-python'))
from coglet_surface.codec import validate


class Surface02Contracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = json.loads((ROOT / 'spec/fixtures/surface02/cases.json').read_text())
        families = ('common', 'abi', 'surface', 'database', 'surface02')
        cls.cddl = '\n'.join((ROOT / f'spec/messages/{family}.cddl').read_text()
                             for family in families)
        schemas = [json.loads((ROOT / f'spec/messages/{family}.schema.json').read_text())
                   for family in families]
        cls.registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in schemas)
        cls.schemas = {name: schema for schema in schemas for name in schema['$defs']}

    def test_surface02_regeneration_current(self):
        for script in ('build_surface02.py', 'build_surface02_fixtures.py'):
            subprocess.run([sys.executable, str(ROOT / 'spec/tests' / script), '--check'], check=True)

    def test_surface02_dual_schema_and_generated_vectors(self):
        self.assertEqual(len(self.cases), len({c['name'] for c in self.cases}))
        for case in self.cases:
            with self.subTest(case=case['name']):
                rule, value = case['type'], case['value']
                schema = self.schemas[rule]
                validator = Draft202012Validator({'$ref': schema['$id'] + '#/$defs/' + rule},
                                                  registry=self.registry)
                self.assertEqual(validator.is_valid(value), case['valid'], 'JSON Schema')
                try:
                    Schema(f'fixture = {rule}\n' + self.cddl).validate_cbor(cbor2.dumps(value))
                    valid = True
                except ValidationError:
                    valid = False
                self.assertEqual(valid, case['valid'], 'independent Ruby CDDL')
                try:
                    validate(rule, value)
                    valid = True
                except ValueError:
                    valid = False
                self.assertEqual(valid, case['valid'], 'generated IR')
                if case['valid']:
                    raw = bytes.fromhex(case['cbor_hex'])
                    self.assertEqual(encode(value), raw)
                    self.assertEqual(decode(raw), value)
        print(f'surface02: {len(self.cases)} independent CDDL/JSON/generated vectors')

    def test_surface02_frozen_rules_are_imports(self):
        schema = self.schemas['s02-manifest']
        self.assertNotIn('capability-name', schema['$defs'])
        self.assertEqual(schema['$defs']['s02-manifest']['properties']['version'], {'const': '0.1'})
        self.assertEqual(schema['$defs']['s02-db-manifest']['properties']['version'], {'const': '0.2'})
        self.assertEqual(schema['$defs']['s02-manifest']['properties']['budgets'],
                         {'$ref': 'abi.schema.json#/$defs/budgets'})


if __name__ == '__main__':
    unittest.main()
