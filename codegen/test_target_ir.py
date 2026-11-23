"""Native targets must not silently ignore additive compiler constraints."""
import unittest
import emit_go
import emit_java


class NativeIRTests(unittest.TestCase):
    def test_new_operations_and_constraints_fail_closed(self):
        text = {'op': 'primitive', 'name': 'tstr'}
        for emit in (emit_go, emit_java):
            cases = [
                {'op': 'intersection', 'items': [text]},
                {'op': 'default', 'item': text, 'value': 'x'},
                {'op': 'tuple', 'items': []},
                {'op': 'array', 'item': text, 'min': 0, 'max': 2},
                {'op': 'map', 'item': text, 'min': 1},
                {'op': 'map', 'item': text, 'max': 1},
                {'op': 'map', 'item': text, 'key': text},
                {'op': 'record', 'fields': [], 'rest': {'item': text, 'min': 0}},
            ]
            for node in cases:
                with self.subTest(target=emit.__name__, node=node):
                    with self.assertRaisesRegex(ValueError, 'unsupported native target IR'):
                        emit.generate({'test': node})
                    # Reject extensions nested in otherwise-supported nodes, too.
                    with self.assertRaisesRegex(ValueError, 'unsupported native target IR'):
                        emit.generate({'test': {'op': 'record', 'fields': [
                            {'name': 'value', 'optional': False, 'type': node}]}})

    def test_op_is_a_legal_rule_name(self):
        for emit in (emit_go, emit_java):
            self.assertTrue(emit.generate({'op': {'op': 'primitive', 'name': 'tstr'}}))
