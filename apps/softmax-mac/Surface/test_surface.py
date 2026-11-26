#!/usr/bin/env python3
"""Actual CDDL, required-vocabulary JSON Schema and SDK-IR differential tests."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
from generate import artifacts, BINDINGS
from schema_validation import validator
sys.path.insert(0, str(ROOT / 'spec/tests'))
from reference_cddl import Schema, ValidationError as CDDLValidationError

codec_spec = importlib.util.spec_from_file_location('product_test_codec', ROOT / 'sdk/surface-python/coglet_surface/codec.py')
codec = importlib.util.module_from_spec(codec_spec)
codec_spec.loader.exec_module(codec)

class ProductTests(unittest.TestCase):
    def test_generation_is_current_and_catalog_closed(self):
        for name, expected in artifacts().items(): self.assertEqual((HERE / name).read_bytes(), expected, name)
        raw = (HERE / 'catalog.v1.json').read_bytes()
        pin = (HERE / 'catalog.v1.sha256').read_text().strip()
        self.assertLessEqual(len(raw), 1_048_576)
        self.assertEqual(hashlib.sha256(raw).hexdigest(), pin)
        catalog = json.loads(raw)
        self.assertEqual(set(catalog), {'version','namespace','rules','schemas','entries','operations'})
        self.assertEqual(catalog['version'], 1)
        self.assertEqual(set(catalog['operations']), {'softmax.app.' + n for n in BINDINGS})
        self.assertEqual(len(catalog['operations']), 13)
        self.assertEqual(catalog['entries'], {'softmax.app':['softmax.app_control'], 'softmax.app_sensitive':['softmax.app_sensitive_control']})
        for name, binding in catalog['operations'].items():
            sensitive = name == 'softmax.app.invoke_sensitive'
            self.assertEqual(binding['entry'], 'softmax.app_sensitive' if sensitive else 'softmax.app')
            self.assertEqual(binding['resources'], ['softmax.app_sensitive_control' if sensitive else 'softmax.app_control'] if binding['direction'] == 'action' else [])
            self.assertTrue(binding['single_owner'])
            for key in ('input','output'):
                if key in binding: self.assertIn(catalog['schemas'][binding[key]],catalog['rules'])
        def closed(node):
            self.assertIn(node['op'], {'record','ref','union','intersection','array','primitive','range','size','literal'})
            if node['op']=='record':
                self.assertEqual(set(node),{'op','fields'})
                for f in node['fields']: closed(f['type'])
            if node['op']=='array':
                self.assertLessEqual(node['max'],262144); closed(node['item'])
            if node['op'] in ('union','intersection'):
                for item in node['items']: closed(item)
            if node['op']=='ref': self.assertIn(node['name'],catalog['rules'])
        for node in catalog['rules'].values(): closed(node)
        print(f'catalog: {len(raw)} bytes, {len(catalog["rules"])} closed rules, 13 bindings, pin {pin}')

    def test_actual_cddl_json_schema_ir_differential(self):
        corpus = json.loads((HERE / 'fixtures.json').read_text())
        source = (HERE / 'softmax-app.cddl').read_text()
        rules = json.loads((HERE / 'softmax-app.ir.json').read_text())
        # Compile each selected CDDL rule once outside subTest: a missing oracle
        # is one infrastructure failure, never hundreds of apparent negatives.
        cddl = {rule: Schema('product-test-root = '+rule+'\n'+source) for rule in rules}
        json_validators = {}
        counts = {True:0,False:0}
        for case in corpus:
            name, rule, valid, value = (case[k] for k in ('name','rule','valid','value'))
            with self.subTest(name=name):
                if rule not in json_validators: json_validators[rule] = validator(rule)
                self.assertEqual(json_validators[rule].is_valid(value),valid,'JSON Schema: '+name)
                try: codec.validate(rule,value,rules); ir_valid=True
                except ValueError: ir_valid=False
                self.assertEqual(ir_valid,valid,'SDK IR: '+name)
                if rule not in cddl: cddl[rule] = Schema('product-test-root = '+rule+'\n'+source)
                try: cddl[rule].validate_cbor(bytes.fromhex(case['cbor_hex'])); cddl_valid=True
                except CDDLValidationError: cddl_valid=False
                self.assertEqual(cddl_valid,valid,'actual CDDL: '+name)
                counts[valid]+=1
        self.assertEqual(len({c['rule'] for c in corpus if c['valid']}), len(rules))
        print(f'actual CDDL / JSON Schema / SDK IR: {len(corpus)} fixtures ({counts[True]} positive, {counts[False]} negative)')

if __name__ == '__main__': unittest.main(verbosity=2)
