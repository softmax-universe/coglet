"""Executable M-B wire-contract checks, not implementation qualification."""
import copy
import json
from pathlib import Path
import unittest

from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from reference_cddl import Schema, ValidationError
from test_common import encode, decode
import build_contract_bindings as build

SPEC = Path(__file__).resolve().parents[1]


class ContractBindings(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.catalog = json.loads((SPEC / 'capabilities/bindings.json').read_text())['operations']
        cls.schemas = {family: json.loads((SPEC / f'messages/{family}.schema.json').read_text())
                       for family in ('common', 'surface', 'speech', 'expression', 'abi')}
        cls.registry = Registry().with_resources((schema['$id'], Resource.from_contents(schema))
                                                 for schema in cls.schemas.values())
        common = (SPEC / 'messages/common.cddl').read_text()
        cls.cddl = {family: (SPEC / f'messages/{family}.cddl').read_text() + '\n' + common
                    for family in ('surface', 'speech', 'expression', 'abi')}

    def valid(self, selected, value):
        family, rule = selected.split(':')
        schema = self.schemas[family]
        valid = Draft202012Validator({'$ref': schema['$id'] + '#/$defs/' + rule},
                                     registry=self.registry).is_valid(value)
        try:
            Schema('selected = ' + rule + '\n' + self.cddl[family]).validate_cbor(encode(value))
            cddl_valid = True
        except ValidationError:
            cddl_valid = False
        self.assertEqual(valid, cddl_valid, (selected, value))
        return valid

    def test_selected_binding_vectors_both_schemas(self):
        vectors = json.loads((SPEC / 'fixtures/surface/bindings.fixture').read_text())
        self.assertEqual(len(vectors), len({v['name'] for v in vectors}))
        for vector in vectors:
            with self.subTest(case=vector['name']):
                selected = self.catalog[vector['operation']][vector['slot']]
                self.assertEqual(self.valid(selected, vector['value']), vector['valid'])
                if vector['valid']:
                    self.assertEqual(encode(vector['value']).hex(), vector['cbor_hex'])
                    self.assertEqual(decode(bytes.fromhex(vector['cbor_hex'])), vector['value'])
        print(f'M-B: {len(vectors)} selected binding vectors; both schema validators')

    def test_catalog_complete_and_exact_direction(self):
        # Independent inventory from the normative operation lists; helper rules
        # such as speech-route are deliberately not operations.
        expected = {
            'event': {'speech.start', 'speech.partial', 'speech.end', 'speech.position',
                      'speech.barge_in', 'speech.tool_call', 'speech.work_intent', 'speech.work_done'},
            'read': {'speech.config', 'speech.heard', 'speech.work_state', 'expression.state'},
            'action': {'speech.speak_chunk', 'speech.stop', 'speech.backchannel',
                       'speech.session_open', 'speech.session_close', 'speech.tool_result',
                       'speech.work_result', 'expression.set_expression'},
            'gateway': {'llm.complete'},
        }
        self.assertEqual(set(self.catalog), set.union(*expected.values()))
        for direction, names in expected.items():
            self.assertEqual({name for name, b in self.catalog.items() if b['direction'] == direction}, names)
        for name, binding in self.catalog.items():
            if binding['direction'] == 'action':
                self.assertEqual(binding['output'], 'surface:empty-map', name)
            if binding['direction'] == 'event':
                self.assertIsNone(binding['output'])
        for name in ('speech.config', 'speech.work_state', 'expression.state'):
            self.assertEqual(self.catalog[name]['input'], 'surface:empty-map')
        self.assertEqual(self.catalog['speech.heard']['input'], 'speech:heard-query')
        self.assertEqual(self.catalog['llm.complete'], dict(entry=None, direction='gateway',
                         input='abi:llm-request', output='abi:llm-reply'))

    def test_generic_envelope_cannot_select_sibling_or_llm_action(self):
        # A shape-valid common action can still violate its selected binding.
        action = dict(id='1'*32, coglet='coglet:'+'b'*64,
                      capability='speech.stop', args={'utterance':'1'*32, 'position_ms':0},
                      deadline=100, correlation_id='2'*32)
        common = self.schemas['common']
        validator = Draft202012Validator({'$ref':common['$id']+'#/$defs/action'}, registry=self.registry)
        self.assertTrue(validator.is_valid(action))
        self.assertTrue(self.valid('speech:speech-start', action['args']))
        self.assertFalse(self.valid(self.catalog[action['capability']]['input'], action['args']))
        action['capability'] = 'llm.complete'
        self.assertTrue(validator.is_valid(action))
        self.assertNotEqual(self.catalog[action['capability']]['direction'], 'action')

    def test_reserved_and_illustrative_names_are_not_surface_bindings(self):
        # Name grammar grants no authority. These cannot acquire a surface
        # binding simply because an envelope or manifest parses.
        reserved = {'runtime.inbox', 'runtime.timer', 'runtime.debug_step',
                    'runtime.action_rejected', 'runtime.receipt', 'action.outcome', 'llm.reply'}
        illustrative = {'sensor.tick', 'motor.move', 'other.write', 'hub.note', 'test.echo', 'speech.speak'}
        self.assertTrue((reserved | illustrative).isdisjoint(self.catalog))
        for operation in self.catalog:
            self.assertFalse(operation.startswith(('runtime.', 'test.')))
        # M1 authority checks are not replaced by this structural exclusion.
        text = (SPEC / 'capabilities/bindings.md').read_text()
        for name in reserved | illustrative:
            self.assertIn('`' + name + '`', text)

    def test_m1_inventory_has_one_disposition_and_owner_per_row(self):
        text = (SPEC.parent / 'docs/reviews/spec-v0-m1-gates.md').read_text()
        rows = [line.split('|')[1:-1] for line in text.splitlines()
                if line.startswith('| ') and '**A**' in line or
                line.startswith('| ') and '**B**' in line]
        expected = {'E-budget', 'E-0001', 'E-0008', 'E-0003', 'H-transform',
                    'W-binding', 'W-llm', 'L-gateway', 'R-retry', 'W-reserved',
                    'R-origin', 'S-speech', 'H-relay-grants', 'H-private', 'H-synthesis', 'F-path'}
        self.assertEqual({row[0].strip().split(':')[0] for row in rows}, expected)
        self.assertEqual(len(rows), len(expected))
        owners = {'t_01a205e4fdee443e', 't_01a205e5cb4956e5', 't_01a205e673b63cb9',
                  't_01a205e73c70350a', 't_01a205e78cbb1f52', 't_01a205e58c02c6ae'}
        for row in rows:
            self.assertEqual(len(row), 4)
            self.assertIn(row[1].strip(), ('**A**', '**B**'))
            if row[1].strip() == '**B**':
                self.assertEqual(sum(owner in row[2] for owner in owners), 1, row)
            else:
                self.assertIn('test_', row[2])

    def test_llm_selected_capability_requires_exact_operation(self):
        value = copy.deepcopy(build.examples()['abi:llm-request'])
        self.assertEqual(value['capability'], 'llm.complete')
        self.assertTrue(self.valid('abi:llm-request', value))
        # Like common action, the reusable ABI shape permits capability-name;
        # selected binding validation must additionally join that discriminator.
        def selected_request(operation, request):
            return (operation in self.catalog and self.catalog[operation]['direction'] == 'gateway'
                    and request['capability'] == operation
                    and self.valid(self.catalog[operation]['input'], request))
        self.assertTrue(selected_request('llm.complete', value))
        value['capability'] = 'other.write'
        self.assertTrue(self.valid('abi:llm-request', value))
        self.assertFalse(selected_request('llm.complete', value))
        self.assertFalse(selected_request('other.write', value))

    def test_generated_catalog_and_fixtures_current(self):
        for relative, expected in build.outputs().items():
            self.assertEqual((SPEC.parent / relative).read_text(), expected, relative)


if __name__ == '__main__':
    unittest.main()
