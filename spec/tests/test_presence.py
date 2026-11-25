"""ADR-0036 shape/version contract checks, NOT router conformance."""
import copy
import unittest

import test_hub


class PresenceShapes(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        test_hub.HubFixtures.setUpClass()
        cls.root = test_hub.HubFixtures.root
        cls.registry = test_hub.HubFixtures.registry
        cls.cddl = test_hub.HubFixtures.cddl
        cls.cases = test_hub.HubFixtures.cases

    validate = test_hub.HubFixtures.validate

    def example(self, name):
        return copy.deepcopy(next(c['value'] for c in self.cases if c['name'] == name))

    def test_presence_methods_require_1_1_request_root(self):
        for name in ['get', 'set', 'request', 'changed', 'check-in']:
            value = self.example('presence-' + name + '-request')
            self.validate('hub-request', value)
            self.validate('hub-frame', value)
            self.validate('hub-v1-request', value, False)
        for c in self.cases:
            if c['valid'] and c['name'] == c['type'] and c['name'].endswith('-request') and not c['name'].startswith('presence-'):
                self.validate('hub-v1-request', c['value'])

    def test_presence_attention_is_only_a_grant(self):
        value = self.example('presence-get-request')
        value['method'] = 'presence.attention'
        self.validate('hub-method', value['method'])
        self.validate('hub-request', value, False)
        self.validate('hub-frame', value, False)

    def test_presence_context_cannot_be_omitted(self):
        for name in ['get', 'set', 'request', 'changed', 'check-in']:
            rule = 'presence-' + name + '-request'
            value = self.example(rule)
            value.pop('context')
            self.validate(rule, value, False)
            self.validate('hub-frame', value, False)

    def test_presence_nested_rejections_at_public_root(self):
        vectors = [
            ('presence-set-request', ('params', 'change', 'base'), -1),
            ('presence-set-request', ('params', 'change', 'limit'), 256),
            ('presence-set-request', ('params', 'change', 'add'), ['bad']),
            ('presence-request-request', ('params', 'request', 'urgency'), 'urgent'),
            ('presence-request-request', ('params', 'request', 'reason'), ''),
            ('presence-changed-request', ('params', 'presence', 'revision'), -1),
            ('presence-check-in-request', ('params', 'check_in', 'dropped'), -1),
            ('presence-check-in-request', ('params', 'check_in', 'status'), {}),
        ]
        for rule, path, bad in vectors:
            value = self.example(rule)
            parent = value
            for key in path[:-1]:
                parent = parent[key]
            parent[path[-1]] = bad
            with self.subTest(rule=rule, path=path):
                for root in [rule, 'hub-request', 'hub-frame']:
                    self.validate(root, value, False)

    def test_presence_bounds_and_cogent_grammar(self):
        for value, valid in [(0, True), (255, True), (-1, False), (256, False), (True, False)]:
            self.validate('presence-limit', value, valid)
        for value, valid in [(10000, True), (86400000, True), (9999, False), (86400001, False)]:
            self.validate('presence-cadence-ms', value, valid)
        self.validate('presence-cogent-id', 'cogent:' + 'd' * 64)
        for value in ['cogent:' + 'd' * 64 + '\n', 'coglet:' + 'd' * 64, 'cogent:' + 'D' * 64]:
            self.validate('presence-cogent-id', value, False)
        good = self.example('presence-check-in')
        message = good['held'][0]
        good['held'] = [message] * 64
        self.validate('presence-check-in', good)
        good['held'].append(message)
        self.validate('presence-check-in', good, False)


if __name__ == '__main__':
    unittest.main()
