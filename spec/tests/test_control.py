"""Handwritten draft fixtures, not kernel/factory implementation conformance.

Use the shared independent reference; no mutation generator or validator changes.
"""
import asyncio
from collections import Counter
import json
from pathlib import Path
import sys
import unittest

import cbor2
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from reference_cddl import Schema, ValidationError
from test_common import decode, encode

SPEC = Path(__file__).resolve().parents[1]
ROOT = SPEC.parent
sys.path.insert(0, str(ROOT / 'codegen'))
from contracts import load
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import run_case, validate_case, validate_impl
from wire import decode as harness_decode, encode as harness_encode

PATHS = {'policy': 'policy', 'promotion': 'messages/promotion', 'tickets': 'messages/tickets'}
IMPORTS = {'policy': ['policy.cddl', 'messages/common.cddl'],
           'promotion': ['messages/promotion.cddl', 'messages/common.cddl'],
           'tickets': ['messages/tickets.cddl', 'messages/promotion.cddl', 'messages/common.cddl']}


def reports_valid(reports, candidate, base=None, revision=None, bind_base=False):
    return (len({r['kind'] for r in reports}) == len(reports)
            and all(r['status'] == 'passed' and r['candidate'] == candidate
                    and (not bind_base or r['base'] == base)
                    and (revision is None or r['policy_revision'] == revision)
                    for r in reports))


def semantic_valid(group, rule, value):
    """Record-local invariants only; signatures/time/grants require future peers."""
    if group == 'policy':
        p = value['probation']
        return (len({g['kind'] for g in value['gates']}) == len(value['gates'])
                and p['health_interval_ms'] <= p['health_timeout_ms'] <= p['window_ms'])
    if group == 'promotion':
        if rule == 'propose':
            return ((value['base'] is None or len(value['base']) == len(value['candidate']))
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
                return (value['generation'] == 0 and all(value[k] is None for k in
                        ('live_commit', 'last_known_good', 'candidate', 'proposal_id')))
    if group == 'tickets':
        if rule == 'ticket' or (rule == 'request' and value['op'] in ('submit', 'amend')):
            ticket = value if rule == 'ticket' else value['ticket']
            paths = ticket['capabilities']['edit_paths']
            return len(paths) == len(set(paths))
        if rule == 'result' and value['state'] == 'done':
            return reports_valid(value['gates'], value['candidate']['commit'])
    return True


class ControlFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        schemas = [json.loads((SPEC / (p + '.schema.json')).read_text())
                   for p in ['messages/common', *PATHS.values()]]
        cls.schemas = dict(zip(['common', *PATHS], schemas))
        cls.registry = Registry().with_resources(
            (s['$id'], Resource.from_contents(s)) for s in schemas)
        for schema in schemas:
            Draft202012Validator.check_schema(schema)

    def test_fixture_shapes_bindings_and_golden_cbor(self):
        totals = Counter()
        for group in PATHS:
            self.assertEqual(load()[group], 'draft-validated')
            schema = self.schemas[group]
            source = '\n'.join((SPEC / p).read_text() for p in IMPORTS[group])
            root = Schema('fixture = ' + group + '\n' + source)
            root_json = Draft202012Validator(schema, registry=self.registry)
            cases = [c for f in sorted((SPEC / 'fixtures' / group).glob('*.json'))
                     if f.name != 'wire-invalid.json' for c in json.loads(f.read_text())]
            self.assertTrue(cases)
            self.assertEqual(len(cases), len({c['name'] for c in cases}))
            for case in cases:
                with self.subTest(group=group, name=case['name']):
                    rule = case['type']
                    entry = 'policy' if group == 'policy' else (
                        ('ticket-' if group == 'tickets' else 'promotion-') + rule)
                    selected = {'$schema': schema['$schema'],
                                '$ref': schema['$id'] + ('' if group == 'policy' else '#/$defs/' + rule)}
                    validator = Draft202012Validator(selected, registry=self.registry)
                    cddl = Schema('fixture = ' + entry + '\n' + source)
                    expected = bool(case['valid'] or case.get('semantic'))
                    self.assertEqual(validator.is_valid(case['value']), expected, 'JSON shape')
                    # Shape negatives may deliberately violate the CBOR profile.
                    raw = cbor2.dumps(case['value'], canonical=True)
                    for oracle, accepted in [(cddl, expected),
                                             (root, root_json.is_valid(case['value']))]:
                        if accepted:
                            oracle.validate_cbor(raw)
                        else:
                            with self.assertRaises(ValidationError):
                                oracle.validate_cbor(raw)
                    if expected:
                        self.assertEqual(semantic_valid(group, rule, case['value']), case['valid'])
                    if case['valid']:
                        golden = bytes.fromhex(case['cbor_hex'])
                        self.assertEqual(raw, golden)
                        self.assertEqual(encode(case['value']), golden)
                        self.assertEqual(decode(golden), case['value'])
                        self.assertEqual(harness_encode(harness_decode(golden)), golden)
                        totals['golden'] += 1
                    elif case.get('semantic'):
                        totals['semantic-invalid'] += 1
                    else:
                        totals['shape-invalid'] += 1
                    totals['fixtures'] += 1
        print('control:', dict(totals))

    def test_handwritten_wire_negatives(self):
        cases = json.loads((SPEC / 'fixtures/policy/wire-invalid.json').read_text())
        for case in cases:
            with self.subTest(name=case['name']):
                raw = bytes.fromhex(case['cbor_hex'])
                with self.assertRaises((ValueError, cbor2.CBORDecodeError, UnicodeError)):
                    decode(raw)
                with self.assertRaises((ValueError, UnicodeError)):
                    harness_decode(raw)
        print('control wire-invalid:', len(cases))

    def test_golden_case_contracts_through_harness(self):
        # Real harness transport, loopback only: no implementation qualification.
        impl = dict(name='draft-loopback', transport='stdio', suites=['draft-control'],
                    command=[sys.executable, str(ROOT / 'conformance/loopback.py')])
        validate_impl(impl)
        count = 0
        for group in PATHS:
            cases = json.loads((SPEC / 'fixtures' / group / 'cases.json').read_text())
            steps = []
            for case in cases:
                if case['valid']:
                    steps.extend([{'send_hex': case['cbor_hex']}, {'expect': case['value']}])
                    count += 1
            timeline = dict(name=group + '-golden-payloads', timeout_ms=10000, steps=steps)
            validate_case(timeline)
            result = asyncio.run(run_case(impl, 'draft-control', timeline))
            self.assertIsNone(result[3], result[3])
        print('control harness loopback goldens:', count)


if __name__ == '__main__':
    unittest.main()
