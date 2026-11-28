"""Presence wire/temporal contract checks; not runtime qualification."""
import copy
import json
from pathlib import Path
import unittest

import test_hub as hub
from presence_oracle import Presence, Rejected, MAX

SPEC = Path(__file__).resolve().parents[1]
COGENT = 'hub:' + 'b' * 64
MEMBERS = ['coglet:' + f'{i:064x}' for i in range(8)]


class PresenceContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        hub.HubFixtures.setUpClass()
        cls.validator = hub.HubFixtures()

    def setUp(self):
        self.router = Presence(COGENT, MEMBERS[0], MEMBERS)
        self.params = dict(cogent=COGENT, base=0, add=[MEMBERS[1]], remove=[])

    def reject(self, code, fn):
        with self.assertRaises(Rejected) as error:
            fn()
        self.assertEqual(error.exception.code, code)

    def test_presence_temporal_fixture_outcomes(self):
        cases = json.loads((SPEC / 'fixtures/hub/presence-transitions.fixture').read_text())
        self.assertEqual(len(cases), len({c['name'] for c in cases}))
        for case in cases:
            with self.subTest(case=case['name']):
                params = dict(case['params'], cogent=COGENT)
                for key in ['add', 'remove']:
                    params[key] = [MEMBERS[i] for i in params[key]]
                if 'attention' in params:
                    params['attention'] = MEMBERS[params['attention']]
                self.validator.validate('presence-set-params', params)
                router = Presence(COGENT, MEMBERS[0], MEMBERS)
                if 'error' in case:
                    self.reject(case['error'], lambda: router.set(params, set(case['grants'])))
                    self.assertEqual(router.state['revision'], 0)
                else:
                    self.assertEqual(router.set(params, set(case['grants']))['revision'], case['revision'])

    def test_presence_reason_utf8_byte_bound(self):
        for reason in ['a' * 1024, 'é' * 512]:
            params = dict(self.params, reason=reason)
            self.validator.validate('presence-set-params', params)
            Presence(COGENT, MEMBERS[0], MEMBERS).set(params, {'presence.set'})
        self.reject('invalid_argument', lambda: self.router.set(
            dict(self.params, reason='é' * 513), {'presence.set'}))

    def test_presence_set_requires_exact_and_attention_grants(self):
        self.reject('permission_denied', lambda: self.router.set(self.params, set()))
        for key, value in [('attention', MEMBERS[0]), ('limit', 4)]:
            params = dict(self.params, **{key: value})
            self.validator.validate('presence-set-params', params)
            self.reject('permission_denied', lambda: self.router.set(params, {'presence.set'}))
            self.assertEqual(self.router.state['revision'], 0)
            self.router.set(params, {'presence.set', 'presence.attention'})
            self.router = Presence(COGENT, MEMBERS[0], MEMBERS)

    def test_presence_set_cas_cap_overlap_foreign_and_noop(self):
        self.router.set(self.params, {'presence.set'})
        before = copy.deepcopy(self.router.state)
        self.reject('conflict', lambda: self.router.set(self.params, {'presence.set'}))
        self.assertEqual(before, self.router.state)
        for params in [dict(self.params, base=1, add=MEMBERS[1:6]),
                       dict(self.params, base=1, remove=[MEMBERS[1]]),
                       dict(self.params, base=1, add=['coglet:' + 'f' * 64]),
                       dict(self.params, base=1, add=[MEMBERS[1]] * 2),
                       dict(self.params, base=1, add=[MEMBERS[0]])]:
            self.reject('invalid_argument', lambda: self.router.set(params, {'presence.set'}))
            self.assertEqual(before, self.router.state)
        noop = dict(self.params, base=1, add=[], remove=[MEMBERS[7]])
        self.assertEqual(self.router.set(noop, {'presence.set'}), before)
        self.router.state['revision'] = MAX
        self.reject('resource_exhausted', lambda: self.router.set(
            dict(self.params, base=MAX, add=[MEMBERS[2]]), {'presence.set'}))

    def test_presence_request_rate_limit_authenticated_source_and_boundary(self):
        params = dict(cogent=COGENT, coglet=MEMBERS[1], urgency='high', reason='Help')
        self.validator.validate('presence-request-params', params)
        self.reject('permission_denied', lambda: self.router.request(params, {'presence.request'}, MEMBERS[2], 0))
        delivered = self.router.request(params, {'presence.request'}, MEMBERS[1], 0)
        self.assertEqual(delivered, {'target': MEMBERS[0], 'params': params})
        self.assertEqual(len(self.router.delivered), 1)
        self.reject('resource_exhausted', lambda: self.router.request(params, {'presence.request'}, MEMBERS[1], 59999))
        self.assertEqual(len(self.router.delivered), 1)
        self.router.request(params, {'presence.request'}, MEMBERS[1], 60000)
        self.assertEqual(len(self.router.delivered), 2)
        self.reject('unavailable', lambda: self.router.request(params, {'presence.request'}, MEMBERS[1], 0))

    def message(self, seq, correlation=None, kind='test.report'):
        source = MEMBERS[1]
        return dict(id=f'{seq:032x}', sender=source,
                    event=dict(source=source, kind=kind, seq=seq, logical_time=seq,
                               payload={}, correlation_id=correlation or f'{seq:032x}'))

    def test_presence_hold_cap_oldest_drops_and_early_check_in_boundaries(self):
        for seq in range(63):
            self.assertIsNone(self.router.message(MEMBERS[1], MEMBERS[0], self.message(seq), 0))
        first = self.router.message(MEMBERS[1], MEMBERS[0], self.message(63), 0)
        self.assertEqual([m['event']['seq'] for m in first['held']], list(range(64)))
        for seq in range(64, 130):
            self.assertIsNone(self.router.message(MEMBERS[1], MEMBERS[0], self.message(seq), 9999))
        self.assertEqual(len(self.router.holds[MEMBERS[1]]), 64)
        self.assertEqual(self.router.dropped[MEMBERS[1]], 2)
        second = self.router.message(MEMBERS[1], MEMBERS[0], self.message(130), 10000)
        self.assertEqual([m['event']['seq'] for m in second['held']], list(range(66, 130)))
        self.assertEqual(second['dropped'], 2)
        self.assertEqual(len(self.router.holds[MEMBERS[1]]), 1)
        self.assertEqual(self.router.dropped[MEMBERS[1]], 0)

    def test_presence_correlated_answers_updates_not_echoed_ids_bypass_holding(self):
        correlation = 'a' * 32
        self.router.message(MEMBERS[1], MEMBERS[0], self.message(0, correlation), 0)
        self.assertEqual(len(self.router.holds[MEMBERS[1]]), 1)
        self.router.call(MEMBERS[1], correlation)
        for seq, kind in [(1, 'memory.result'), (2, 'ticket.update')]:
            self.router.message(MEMBERS[1], MEMBERS[0], self.message(seq, correlation, kind), 0)
        self.assertEqual(len(self.router.delivered), 2)
        self.assertEqual(len(self.router.holds[MEMBERS[1]]), 1)
        self.router.set(dict(self.params, add=[], attention=MEMBERS[2]), {'presence.set', 'presence.attention'})
        check = self.router.check_in(MEMBERS[1])
        self.assertEqual(len(check['held']), 1)
        self.assertEqual(self.router.state['attention'], MEMBERS[2])
        self.router.message(MEMBERS[1], MEMBERS[2], self.message(3, correlation), 0)
        self.assertEqual(len(self.router.holds[MEMBERS[1]]), 1)  # Old attention's call is not new authority.

    def test_presence_ended_or_revoked_call_cannot_bypass(self):
        for terminal in ['ended', 'revoked']:
            router = Presence(COGENT, MEMBERS[0], MEMBERS)
            correlation = 'e' * 32
            router.call(MEMBERS[1], correlation)
            router.end_call(MEMBERS[1], correlation)
            router.message(MEMBERS[1], MEMBERS[0], self.message(0, correlation, 'ticket.update'), 0)
            self.assertEqual(len(router.delivered), 0, terminal)
            self.assertEqual(len(router.holds[MEMBERS[1]]), 1, terminal)

    def test_presence_individual_held_record_encoded_size_boundary(self):
        from test_common import encode
        message = self.message(0)
        message['event']['payload'] = {'text': ''}
        # A 32-bit CBOR string-length header is four bytes wider than empty.
        overhead = len(encode(message)) + 4
        message['event']['payload']['text'] = 'a' * (524288 - overhead)
        self.assertEqual(len(encode(message)), 524288)
        self.router.message(MEMBERS[1], MEMBERS[0], message, 0)
        bad = copy.deepcopy(message)
        bad['event']['payload']['text'] += 'a'
        self.assertEqual(len(encode(bad)), 524289)
        self.reject('resource_exhausted', lambda: self.router.message(MEMBERS[1], MEMBERS[0], bad, 0))
        self.assertEqual(len(self.router.holds[MEMBERS[1]]), 1)

    def test_presence_positive_negative_vectors_and_recursive_root_closure(self):
        cases = json.loads((SPEC / 'fixtures/hub/cases.json').read_text())
        for case in cases:
            if not case['type'].startswith('presence-'):
                continue
            self.validator.validate(case['type'], case['value'], case['valid'])
        requests = [c for c in cases if c['valid'] and c['name'] == c['type'] and c['type'].startswith('presence-')
                    and c['type'].endswith('-request')]
        self.assertEqual(len(requests), 5)
        def maps(value, path=()):
            if isinstance(value, dict):
                yield path
                for key, child in value.items():
                    yield from maps(child, path + (key,))
            elif isinstance(value, list):
                for key, child in enumerate(value):
                    yield from maps(child, path + (key,))
        for case in requests:
            good = case['value']
            for rule in [case['type'], 'hub-request', 'hub-frame']:
                self.validator.validate(rule, good)
                for path in maps(good):
                    # Common event payload is deliberately open.
                    if path and path[-1] == 'payload':
                        continue
                    bad = copy.deepcopy(good)
                    parent = bad
                    for key in path:
                        parent = parent[key]
                    parent['unknown'] = True
                    with self.subTest(rule=rule, path=path):
                        self.validator.validate(rule, bad, False)
                bad = copy.deepcopy(good)
                del bad['context']
                self.validator.validate(rule, bad, False)


if __name__ == '__main__':
    unittest.main()
