"""Draft control fixtures: shape and local bindings, NOT implementation conformance.

Semantic negatives are valid JSON Schema shapes with invalid cross-field values.
No network resolution is allowed; every imported schema is registered locally.
"""
import json
from pathlib import Path
import unittest

from jsonschema import Draft202012Validator
from referencing import Registry, Resource

from test_common import decode, encode

SPEC = Path(__file__).resolve().parents[1]
SCHEMAS = {
    'policy': SPEC / 'policy.schema.json',
    'promotion': SPEC / 'messages/promotion.schema.json',
    'tickets': SPEC / 'messages/tickets.schema.json',
}


def reports_valid(reports, candidate, base=None, revision=None, bind_base=False):
    kinds = [r['kind'] for r in reports]
    return (len(kinds) == len(set(kinds))
            and all(r['status'] == 'passed' and r['candidate'] == candidate
                    and (not bind_base or r['base'] == base)
                    and (revision is None or r['policy_revision'] == revision)
                    for r in reports))


def semantic_valid(group, rule, value):
    """Only invariants observable from this record; authority/time need a kernel."""
    if group == 'policy':
        kinds = [g['kind'] for g in value['gates']]
        p = value['probation']
        return (len(kinds) == len(set(kinds))
                and p['health_interval_ms'] <= p['health_timeout_ms'] <= p['window_ms'])
    if group == 'promotion':
        if rule == 'propose':
            commits = [value['candidate'], value['base']]
            return (all(c is None or len(c) == len(value['candidate']) for c in commits)
                    and (value['base'] is not None or value['expected_generation'] == 0)
                    and reports_valid(value['gates'], value['candidate'], value['base'],
                                      value['policy_revision'], bind_base=True))
        if rule == 'response' and value['ok'] and value['op'] == 'status':
            return semantic_valid(group, 'status', value['result'])
        if rule == 'status':
            state, p = value['state'], value['probation']
            if (state == 'probation') != (p is not None):
                return False
            if state in ('proposed', 'activating', 'probation'):
                if value['candidate'] is None or value['proposal_id'] is None:
                    return False
            if state == 'probation':
                return (p['expires_at_ms'] > p['started_at_ms']
                        and value['live_commit'] == value['candidate'])
            if state == 'stable':
                return (value['live_commit'] is not None
                        and value['live_commit'] == value['last_known_good']
                        and value['candidate'] is None and value['proposal_id'] is None)
            if state == 'empty':
                return (value['generation'] == 0
                        and all(value[k] is None for k in ('live_commit', 'last_known_good',
                                                          'candidate', 'proposal_id')))
        return True
    if group == 'tickets' and rule == 'result' and value['state'] == 'done':
        return reports_valid(value['gates'], value['candidate']['commit'])
    return True


class ControlFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        all_paths = [SPEC / 'messages/common.schema.json', *SCHEMAS.values()]
        cls.schemas = {p: json.loads(p.read_text()) for p in all_paths}
        cls.registry = Registry().with_resources(
            (s['$id'], Resource.from_contents(s)) for s in cls.schemas.values())
        for schema in cls.schemas.values():
            Draft202012Validator.check_schema(schema)

    def test_fixture_shapes_bindings_and_golden_cbor(self):
        for group, path in SCHEMAS.items():
            schema = self.schemas[path]
            cases = json.loads((SPEC / 'fixtures' / group / 'cases.json').read_text())
            self.assertTrue(cases)
            self.assertEqual(len(cases), len({c['name'] for c in cases}))
            for case in cases:
                with self.subTest(group=group, case=case['name']):
                    selected = dict(schema)
                    if case['type'] != 'policy':
                        selected.pop('oneOf', None)
                        selected.pop('anyOf', None)
                        selected['$ref'] = '#/$defs/' + case['type']
                    validator = Draft202012Validator(selected, registry=self.registry)
                    errors = list(validator.iter_errors(case['value']))
                    shape_ok = not errors
                    expected_shape = case['valid'] or case.get('semantic', False)
                    self.assertEqual(shape_ok, expected_shape,
                                     '\n'.join(e.message for e in errors))
                    if shape_ok:
                        self.assertEqual(semantic_valid(group, case['type'], case['value']),
                                         case['valid'], 'cross-field invariant')
                    if case['valid']:
                        raw = bytes.fromhex(case['cbor_hex'])
                        self.assertEqual(decode(raw), case['value'])
                        self.assertEqual(encode(case['value']), raw)
                        self.assertEqual(encode(decode(raw)), raw)
            print(f'{group}: {len(cases)} fixtures; '
                  f'{sum(c["valid"] for c in cases)} golden round-trips; '
                  f'{sum(c.get("semantic", False) for c in cases)} semantic negatives')

    def test_schema_roots_accept_all_valid_messages(self):
        for group, path in SCHEMAS.items():
            validator = Draft202012Validator(self.schemas[path], registry=self.registry)
            cases = json.loads((SPEC / 'fixtures' / group / 'cases.json').read_text())
            for case in cases:
                if case['valid'] and case['type'] not in ('status', 'gate-report', 'ticket', 'receipt'):
                    with self.subTest(group=group, case=case['name']):
                        validator.validate(case['value'])

    def test_every_operation_has_positive_fixture(self):
        for group, required in [('promotion', {'propose', 'promote', 'rollback', 'status'}),
                                ('tickets', {'submit', 'update', 'answer', 'amend', 'cancel', 'result'})]:
            cases = json.loads((SPEC / 'fixtures' / group / 'cases.json').read_text())
            ops = {c['value'].get('op') for c in cases if c['valid']}
            self.assertTrue(required <= ops)


if __name__ == '__main__':
    unittest.main()
