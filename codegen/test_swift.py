"""Swift emitter regressions. macOS conformance CI must run the compile tests."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from generate import parse
from swift import generate_swift

ROOT = Path(__file__).resolve().parents[1]


class SwiftEmitterTests(unittest.TestCase):
    def test_type_identifier_collisions_fail_closed(self):
        for cddl in ('a-b = tstr\na_b = bool',
                     'CBORValue = tstr',
                     'swift = tstr', 'any = tstr', 'set = tstr',
                     'equatable = tstr', 'sendable = tstr', 'foundation = tstr',
                     'thing = {choice: tstr / bool}\nthing-choice = tstr',
                     'thing = {name: tstr} / {name: bool}\nthing-variant1 = bool'):
            with self.subTest(cddl=cddl), self.assertRaisesRegex(ValueError, 'collision'):
                generate_swift(parse(cddl))

    def test_field_identifier_collisions_fail_closed(self):
        for fields in ('a-b: tstr, a_b: bool', 'self: tstr, self_: bool',
                       'class: tstr, class: bool', 'cborValue: tstr, cborValue_: bool'):
            with self.subTest(fields=fields), self.assertRaisesRegex(ValueError, 'collision|duplicate field'):
                generate_swift(parse('thing = {' + fields + '}'))
        for key in ('"123"', '"a b"', '"_"', '"a`b"'):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'identifier'):
                generate_swift(parse('thing = {' + key + ': tstr}'))

    def test_nonproductive_and_struct_only_recursion_fail_closed(self):
        for cddl in ('cycle = cycle', 'a = b\nb = a',
                     'cycle = cycle / null', 'cycle = { ? next: cycle }',
                     'tree = [* tree]', 'a = [* b]\nb = [* a]'):
            with self.subTest(cddl=cddl), self.assertRaisesRegex(ValueError, 'recursive type cycle'):
                generate_swift(parse(cddl))
        # Recursive containers behind an indirect enum are productive.
        self.assertIn('indirect enum Tree', generate_swift(parse('tree = null / [* tree]')))
        self.assertIn('struct Tree', generate_swift(parse('tree = {children: [* tree]}')))

    def test_field_shadowing_all_generated_types_fails_closed(self):
        for cddl in ('child = {value: tstr}\nthing = {Child: child}',
                     'thing = {ThingChoice: tstr, choice: tstr / bool}',
                     'thing = {Thing: tstr}',
                     'thing = {ThingChoice: tstr}\nother = {choice: tstr / bool}\nthing-choice = bool'):
            with self.subTest(cddl=cddl), self.assertRaisesRegex(ValueError, 'collision'):
                generate_swift(parse(cddl))

    def test_unknown_ir_fails_closed(self):
        with self.assertRaises(ValueError):
            generate_swift({'thing': {'op': 'future-node'}})
        with self.assertRaises(ValueError):
            generate_swift({'thing': {'op': 'primitive', 'name': 'tstr', 'future': 1}})

    def test_no_normalizing_equatable_on_generated_types(self):
        source = generate_swift(parse('record = {value: tstr}\nchoice = tstr / bool'))
        self.assertNotIn('Equatable', source)

    @unittest.skipUnless(shutil.which('swiftc'), 'Swift compile regression needs swiftc (CI: swift-codegen)')
    def test_expanded_ir_compiles_and_enforces_constraints(self):
        text = {'op': 'primitive', 'name': 'tstr'}
        boolean = {'op': 'primitive', 'name': 'bool'}
        literal = lambda value: {'op': 'literal', 'value': value}
        rules = {
            'demo::text': text,
            'alias': {'op': 'ref', 'name': 'demo::text'},
            'bounded': {'op': 'array', 'min': 1, 'max': 2, 'item': text},
            'mapping': {'op': 'map', 'min': 1, 'max': 2, 'key': {'op': 'regexp', 'pattern': '^[a-z]+$'}, 'item': boolean},
            'empty': {'op': 'intersection', 'items': [literal(None), literal(False)]},
            'sized': {'op': 'intersection', 'items': [text, {'op': 'size', 'min': 1, 'max': 2}]},
            'defaulted': {'op': 'default', 'item': text, 'value': 'metadata'},
            'sequence': {'op': 'tuple', 'items': [{'type': text, 'min': 0, 'max': 2}, {'type': literal('end'), 'min': 1, 'max': 1}]},
            'open': {'op': 'record', 'fields': [{'name': 'fixed', 'optional': False, 'type': boolean}], 'rest': {'min': 1, 'max': 2, 'key': {'op': 'regexp', 'pattern': '^extra[a-z]$'}, 'item': text}},
        }
        wrapped = {'op': 'ref', 'name': 'tree'}
        for _ in range(4):
            wrapped = {'op': 'default', 'value': None, 'item': {'op': 'intersection', 'items': [wrapped]}}
        rules['tree'] = {'op': 'union', 'items': [literal(None), {'op': 'array', 'min': 0, 'item': wrapped}]}
        main = '''import Foundation
        func accepted(_ rule: Swift.String, _ value: CBORValue) throws {
            let raw = try CBORCodec.encode(rule: rule, value: value)
            let rt = try GeneratedSchema.typedRoundTrip(rule: rule, bytes: raw)
            precondition(raw == rt)
        }
        func rejected(_ rule: Swift.String, _ value: CBORValue) throws {
            let raw = try CBORCodec.encodeProfile(value)
            do { _ = try CBORCodec.decode(rule: rule, bytes: raw); fatalError("accepted " + rule) } catch {}
            do { _ = try CBORCodec.encode(rule: rule, value: value); fatalError("encoded " + rule) } catch {}
        }
        try accepted("alias", .text("x"))
        try accepted("demo::text", .text("x"))
        try accepted("bounded", .array([.text("a"), .text("b")]))
        try rejected("bounded", .array([]))
        try rejected("bounded", .array([.text("a"), .text("b"), .text("c")]))
        try accepted("mapping", .map([.init("a", .boolean(true))]))
        try rejected("mapping", .map([]))
        try rejected("mapping", .map([.init("A", .boolean(true))]))
        try rejected("mapping", .map([.init("a", .null)]))
        try rejected("mapping", .map([.init("a", .boolean(true)), .init("b", .boolean(true)), .init("c", .boolean(true))]))
        try rejected("empty", .null)
        try rejected("empty", .boolean(false))
        try accepted("sized", .text("é"))
        try rejected("sized", .text("abc"))
        try accepted("defaulted", .text("original"))
        try rejected("defaulted", .null)
        try accepted("sequence", .array([.text("end")]))
        try accepted("sequence", .array([.text("a"), .text("b"), .text("end")]))
        try rejected("sequence", .array([.text("a")]))
        try rejected("sequence", .array([.text("a"), .text("b"), .text("c"), .text("end")]))
        try accepted("open", .map([.init("fixed", .boolean(true)), .init("extraa", .text("x"))]))
        try rejected("open", .map([.init("fixed", .boolean(true))]))
        try rejected("open", .map([.init("extraa", .text("x"))]))
        try rejected("open", .map([.init("fixed", .boolean(true)), .init("other", .text("x"))]))
        try rejected("open", .map([.init("fixed", .boolean(true)), .init("extraa", .boolean(true))]))
        try rejected("open", .map([.init("fixed", .boolean(true)), .init("extraa", .text("a")), .init("extrab", .text("b")), .init("extrac", .text("c"))]))
        var deep = CBORValue.null
        for _ in 0..<64 { deep = .array([deep]) }
        try accepted("tree", deep)
        do { _ = try CBORCodec.encode(rule: "tree", value: .array([deep])); fatalError("depth overflow accepted") } catch {}
        print("expanded IR passed")
        '''
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            temp = Path(temp)
            (temp / 'Types.swift').write_text(generate_swift(rules))
            (temp / 'main.swift').write_text(main)
            result = subprocess.run(['swiftc', str(ROOT / 'sdk/surface-swift/Sources/CogletTypes/CBOR.swift'), str(temp / 'Types.swift'), str(temp / 'main.swift'), '-o', str(temp / 'test')], text=True, capture_output=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(temp / 'test')], text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('expanded IR passed', result.stdout)

    @unittest.skipUnless(shutil.which('swiftc'), 'Swift compile regression needs swiftc (CI: swift-codegen)')
    def test_keywords_wire_mapping_and_inline_types_compile_and_run(self):
        cddl = '''thing = {
          class: tstr, switch: tstr, let: tstr, var: tstr, protocol: tstr,
          associatedtype: tstr, some: tstr, any: tstr, actor: tstr,
          async: tstr, await: tstr, inout: tstr, repeat: tstr,
          self: tstr, Self: tstr, super: tstr, init: tstr, deinit: tstr,
          subscript: tstr, Type: tstr, Protocol: tstr, type: tstr,
          foo-bar: tstr, cborValue: tstr, schemaRule: tstr, depth: tstr,
          Swift: tstr, CBORCodec: tstr, CBORValue: tstr,
          ? optional: null,
          nested: {default: bool},
          choice: "one" / "two"
        }
        text = tstr
        integer = -8..8
        number = 0..18446744073709551615
        branch = {children: [* branch]}
        '''
        rules = parse(cddl)
        fields = rules['thing']['fields']
        # Build the source through wire values: this exercises generated init,
        # escaped stored properties AND exact original field names at encode.
        from swift import quote
        entries = ', '.join('.init(' + quote(f['name']) + ', .text("ok"))'
                            for f in fields if f['name'] not in ('nested', 'choice', 'optional'))
        main = '''import Foundation
        let value = CBORValue.map([''' + entries + ''',
            .init("optional", .null), .init("nested", .map([.init("default", .boolean(true))])),
            .init("choice", .text("one"))])
        let bytes = try CBORCodec.encode(rule: "thing", value: value)
        let branch = Branch(children: [Branch(children: [])])
        let branchBytes = try branch.toCBOR()
        let branchRoundTrip = try Branch.fromCBOR(branchBytes).toCBOR()
        precondition(branchRoundTrip == branchBytes)
        let typed = try Thing.fromCBOR(bytes)
        precondition(typed.`class` == "ok")
        precondition(typed.`self_` == "ok")
        precondition(typed.`foo_bar` == "ok")
        precondition(typed.`cborValue_` == "ok")
        precondition(typed.`Swift_` == "ok")
        precondition(typed.`CBORCodec_` == "ok")
        precondition(typed.`CBORValue_` == "ok")
        precondition(typed.`optional` == .null)
        precondition(try typed.toCBOR() == bytes)
        let absent = CBORValue.map(try value.requireMap().filter { $0.key != "optional" })
        let absentBytes = try CBORCodec.encode(rule: "thing", value: absent)
        precondition(try Thing.fromCBOR(absentBytes).toCBOR() == absentBytes)
        precondition(try GeneratedSchema.typedRoundTrip(rule: "thing", bytes: bytes) == bytes)
        do {
            _ = try ThingChoice.variant1("two").toCBOR()
            fatalError("wrong literal branch accepted")
        } catch {}
        print("Swift keyword wire mapping passed")
        '''
        # Swift precondition's autoclosure cannot throw.
        main = main.replace('precondition(try typed.toCBOR() == bytes)',
                            'let encoded = try typed.toCBOR(); precondition(encoded == bytes)')
        main = main.replace('precondition(try Thing.fromCBOR(absentBytes).toCBOR() == absentBytes)',
                            'let absentEncoded = try Thing.fromCBOR(absentBytes).toCBOR(); precondition(absentEncoded == absentBytes)')
        main = main.replace('precondition(try GeneratedSchema.typedRoundTrip(rule: "thing", bytes: bytes) == bytes)',
                            'let rt = try GeneratedSchema.typedRoundTrip(rule: "thing", bytes: bytes); precondition(rt == bytes)')
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            temp = Path(temp)
            (temp / 'Types.swift').write_text(generate_swift(rules))
            (temp / 'main.swift').write_text(main)
            cmd = ['swiftc', str(ROOT / 'sdk/surface-swift/Sources/CogletTypes/CBOR.swift'),
                   str(temp / 'Types.swift'), str(temp / 'main.swift'), '-o', str(temp / 'test')]
            result = subprocess.run(cmd, text=True, capture_output=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(temp / 'test')], text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('Swift keyword wire mapping passed', result.stdout)
