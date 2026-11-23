import importlib.util
import json
from pathlib import Path
import sys
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / 'sdk/surface-python'))
from cddl import parse, parse_files
from generate import generate


class ConstructTests(unittest.TestCase):
    def test_groups_and_socket(self):
        rules = parse('env = (id: tstr, ? flag: bool)\n$$g //= (op: "a")\n$$g //= (op: "b")\nx = {env, $$g}')
        self.assertNotIn('env', rules)
        self.assertEqual(len(rules['x']['items']), 2)
        self.assertEqual([f['name'] for f in rules['x']['items'][0]['fields']], ['id', 'flag', 'op'])

    def test_inline_choice(self):
        node = parse('x = {(op: "a", n: tstr) // (op: "b", n: bool)}')['x']
        self.assertEqual(node['op'], 'union')

    def test_controls(self):
        rules = parse('x = tstr .size (1..4) .regexp "[a-z]+"\nn = nil .and false\nd = tstr .default "x"')
        self.assertEqual(rules['x']['op'], 'intersection')
        self.assertEqual(rules['n']['items'][0]['value'], None)
        self.assertEqual(rules['d']['value'], 'x')

    def test_bounds_tuple(self):
        rules = parse('a = [* 64 tstr]\nb = [1*3 bool]\nc = ["a", ? bool, * 2 tstr]\nd = {* 0 tstr => tstr}')
        self.assertEqual(rules['a']['max'], 64)
        self.assertEqual(rules['b']['min'], 1)
        self.assertEqual(rules['c']['op'], 'tuple')
        self.assertEqual(rules['d']['max'], 0)

    def test_namespace(self):
        rules = parse_files({'common': 'id = tstr', 'a': 'wire = {id: id}', 'b': 'wire = [* id]'})
        self.assertNotIn('wire', rules)
        self.assertEqual(rules['a::wire']['fields'][0]['type']['name'], 'common::id')
        self.assertIn('id', rules)
        with self.assertRaisesRegex(ValueError, 'ambiguous'):
            parse_files({'a': 'x = tstr', 'b': 'x = bool', 'c': 'y = x'})

    def test_loud_failure(self):
        for text in ('x = tstr .foo 1', 'x = [3*2 tstr]', 'x = 9..1', 'x = {a: bool, a: bool}',
                     'x = tstr .regexp "["', 'x = tstr .regexp "(?=x)"', 'x = bstr .cbor any',
                     'g = (g, x: tstr)\nx = {g}'):
            with self.subTest(text=text), self.assertRaises(ValueError): parse(text)

    def test_identifier_collisions(self):
        for text in ('a-b = tstr\na_b = bool', 'a = {foo-bar: tstr, foo_bar: bool}',
                     'a = {from: tstr, from_: bool}', 'a = {to_cbor: tstr}'):
            with self.subTest(text=text), self.assertRaises(ValueError): generate(parse(text))
        output = generate(parse('a = {type: tstr, from: tstr, fn: tstr, "foo-bar": tstr}'))
        py = output['sdk/surface-python/coglet_surface/generated.py']
        compile(py, '<generated>', 'exec')
        self.assertIn('metadata={"wire": \'foo-bar\'}', py)
        self.assertIn('pub fn_: String', output['crates/coglet-types/src/generated.rs'])

    def test_python_runtime_constructs(self):
        from coglet_surface.codec import validate, encode_profile, decode_profile
        rules = parse('a = [* 2 tstr]\nb = {* 0 tstr => tstr}\nc = ["a", ? bool, * 2 tstr]\nd = nil .and false\ne = tstr .default "a"\nf = {a: tstr, * tstr => nil .and false}')
        cases = [('a', ['a', 'b'], True), ('a', ['a'] * 3, False), ('b', {}, True), ('b', {'x': 'a'}, False),
                 ('c', ['a', True, 'b', 'c'], True), ('c', ['a', 'b'], True), ('c', ['a', False, 'b', 'c', 'd'], False),
                 ('d', None, False), ('d', False, False), ('e', 'b', True), ('e', None, False),
                 ('f', {'a': 'b'}, True), ('f', {'a': 'b', 'x': None}, False)]
        for rule, value, valid in cases:
            with self.subTest(rule=rule, value=value):
                if valid:
                    validate(rule, value, rules)
                    validate(rule, decode_profile(encode_profile(value)), rules)
                else:
                    with self.assertRaises(ValueError): validate(rule, value, rules)


if __name__ == '__main__': unittest.main()
