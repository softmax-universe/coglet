"""Independent CDDL scalar/container regressions; no generated oracle."""
import unittest
import os
from pathlib import Path
import subprocess
import sys
import json

import cbor2
from reference_cddl import Schema, ValidationError, ReferenceError, _worker


class ReferenceCddlTests(unittest.TestCase):
    def test_union_scalar_is_not_an_array(self):
        for source in ['root = {kind: "x" / "y"}',
                       'root = {kind: "x"} / {kind: "y"}']:
            with self.subTest(source=source):
                Schema(source).validate_cbor(cbor2.dumps({'kind': 'x'}))
                with self.assertRaises(ValidationError):
                    Schema(source).validate_cbor(cbor2.dumps({'kind': [1]}))

    def test_common_recursive_cddl_json_agreement(self):
        from cddl_differential import audit, mutations
        value = {'a': [{'b': 'x'}]}
        cases = list(mutations(value))
        self.assertTrue(any(path == ('a', 0, 'b') and v['a'][0]['b'] == [1] for path, v in cases))
        self.assertEqual(value, {'a': [{'b': 'x'}]})
        result = audit(Path(__file__).resolve().parents[1], 'common')
        self.assertEqual(result['mismatches'], 0, result['findings'][:5])
        self.assertGreater(result['comparisons'], 2000)
        self.assertGreater(result['accepted_controls'], 100)

    def test_differential_explicit_import(self):
        import tempfile
        from cddl_differential import audit
        with tempfile.TemporaryDirectory() as td:
            spec = Path(td)
            (spec / 'messages').mkdir()
            (spec / 'fixtures/tickets').mkdir(parents=True)
            (spec / 'messages/tickets.cddl').write_text('root = imported\n')
            (spec / 'messages/promotion.cddl').write_text('imported = "ok"\n')
            (spec / 'messages/common.cddl').write_text('common = uint\n')
            (spec / 'messages/tickets.schema.json').write_text(json.dumps({
                '$schema': 'https://json-schema.org/draft/2020-12/schema',
                '$id': 'https://example.test/tickets', 'const': 'ok',
                '$defs': {'root': {'const': 'ok'}}}))
            (spec / 'fixtures/tickets/cases.json').write_text(json.dumps([
                {'name': 'import', 'type': 'root', 'valid': True, 'value': 'ok'}]))
            result = audit(spec, 'tickets', includes=['promotion'])
            self.assertEqual(result['mismatches'], 0)
            self.assertGreater(result['accepted_controls'], 0)

    def test_zero_occurrence_map_does_not_consume_keys(self):
        for source in ['root = {*0 tstr => tstr}', 'root = {0*0 forbidden: tstr}']:
            schema = Schema(source)
            schema.validate_cbor(cbor2.dumps({}))
            with self.subTest(source=source), self.assertRaises(ValidationError):
                schema.validate_cbor(cbor2.dumps({'forbidden': 'x'}))
        # Zero occurrences means no consumption, not a global prohibition:
        # a later group may legitimately consume the same key.
        Schema('root = {*0 tstr => tstr, actual: tstr}').validate_cbor(
            cbor2.dumps({'actual': 'x'}))

    def test_nested_scalar_container_substitutions(self):
        # Strings, primitives, ranges and maps all went through Rust's
        # scalar-on-array dispatch; test every depth, including array members.
        types = [('"x" / "y"', 'x'), ('tstr / uint', 'x'),
                 ('uint / bool', 2), ('0..4 / tstr', 2),
                 ('{a: tstr} / {b: uint}', {'a': 'x'})]
        count = 0
        for rule, valid in types:
            for template, wrap in [
                    ('VALUE', lambda x: x),
                    ('{item: VALUE}', lambda x: {'item': x}),
                    ('[* VALUE]', lambda x: [x]),
                    ('{items: [* {item: VALUE}]}', lambda x: {'items': [{'item': x}]})]:
                schema = Schema('root = ' + template.replace('VALUE', '(' + rule + ')'))
                schema.validate_cbor(cbor2.dumps(wrap(valid)))
                for bad in [[], [1], [[]], [[1]], [{'a': 'x'}]]:
                    with self.subTest(rule=rule, template=template, bad=bad):
                        with self.assertRaises(ValidationError):
                            schema.validate_cbor(cbor2.dumps(wrap(bad)))
                    count += 1
        print(f'reference: {count} nested scalar/container negatives; 20 legitimate nesting controls')

    def test_legitimate_array_choices_and_profile_maps(self):
        schema = Schema('root = {items: [* (tstr / [* uint] / {a: bool})]}')
        for value in [[], ['x'], [[1, 2]], [{'a': True}], ['x', [], [1], {'a': False}]]:
            schema.validate_cbor(cbor2.dumps({'items': value}))
        with self.assertRaises(ValidationError):
            schema.validate_cbor(cbor2.dumps({'items': [[False]]}))
        schema = Schema('root = {* tstr => value}\nvalue = tstr / int / bool / nil / [* value] / {* tstr => value}')
        schema.validate_cbor(cbor2.dumps({'x': [1, 'x', None, {'y': False}], 'y': {'z': []}}))

    def test_byte_strings_remain_distinct_from_text(self):
        for source, good, bad in [('root = tstr', 'x', b'x'), ('root = bstr', b'x', 'x')]:
            schema = Schema(source)
            schema.validate_cbor(cbor2.dumps(good))
            with self.assertRaises(ValidationError):
                schema.validate_cbor(cbor2.dumps(bad))

    def test_invalid_schema_and_cbor_are_tool_errors(self):
        with self.assertRaises(ReferenceError):
            Schema('root = {')
        schema = Schema('root = tstr')
        for raw in [b'', b'\xff', cbor2.dumps('x') + cbor2.dumps('y')]:
            with self.subTest(raw=raw), self.assertRaises(ReferenceError):
                schema.validate_cbor(raw)
        schema.validate_cbor(cbor2.dumps('x'))

    @unittest.skipUnless(sys.platform == 'linux', 'Linux RLIMIT_AS stress check')
    def test_bounded_memory_recursive_negatives(self):
        if os.environ.get('REFERENCE_CDDL_MEMORY_STRESS') != '1':
            result = subprocess.run([sys.executable, __file__,
                'ReferenceCddlTests.test_bounded_memory_recursive_negatives'],
                env=dict(os.environ, REFERENCE_CDDL_MEMORY_STRESS='1'),
                capture_output=True, text=True, timeout=90)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('600 verdicts under 512MiB', result.stdout)
            return
        import resource
        resource.setrlimit(resource.RLIMIT_AS, (512 * 1024**2,) * 2)
        spec = Path(__file__).resolve().parents[1]
        schema = Schema((spec / 'messages/common.cddl').read_text())
        case = json.loads((spec / 'fixtures/common/cases.json').read_text())[0]
        value = dict(case['value'], payload={'nested': [True, None, -1, b'wrong']})
        raw = cbor2.dumps(value)
        for _ in range(600):
            with self.assertRaises(ValidationError):
                schema.validate_cbor(raw)
        print('600 verdicts under 512MiB')

    def test_cached_parser_has_no_cross_request_validation_state(self):
        schema = Schema('root = {kind: "x" / "y"}')
        for _ in range(10):
            for source in ['root = [* uint]', 'root = {a: bool}']:
                Schema(source)
            for value, valid in [({'kind': [1]}, False), ({'kind': 'y'}, True),
                                 ({'kind': {'x': 1}}, False), ({'kind': 'x'}, True)]:
                if valid:
                    schema.validate_cbor(cbor2.dumps(value))
                else:
                    with self.assertRaises(ValidationError):
                        schema.validate_cbor(cbor2.dumps(value))
        _worker.close()
        schema.validate_cbor(cbor2.dumps({'kind': 'x'}))


if __name__ == '__main__':
    unittest.main()
