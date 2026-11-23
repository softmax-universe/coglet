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
                     'swift = tstr', 'any = tstr',
                     'thing = {choice: tstr / bool}\nthing-choice = tstr',
                     'thing = {name: tstr} / {name: bool}\nthing-variant1 = bool'):
            with self.subTest(cddl=cddl), self.assertRaisesRegex(ValueError, 'collision'):
                generate_swift(parse(cddl))

    def test_field_identifier_collisions_fail_closed(self):
        for fields in ('a-b: tstr, a_b: bool', 'self: tstr, self_: bool',
                       'class: tstr, class: bool', 'cborValue: tstr, cborValue_: bool'):
            with self.subTest(fields=fields), self.assertRaisesRegex(ValueError, 'collision'):
                generate_swift(parse('thing = {' + fields + '}'))
        for key in ('"123"', '"a b"', '"_"', '"a`b"'):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'identifier'):
                generate_swift(parse('thing = {' + key + ': tstr}'))

    def test_unknown_ir_fails_closed(self):
        with self.assertRaises(ValueError):
            generate_swift({'thing': {'op': 'future-node'}})

    @unittest.skipUnless(shutil.which('swiftc'), 'Swift compile regression runs in macOS CI')
    def test_keywords_wire_mapping_and_inline_types_compile_and_run(self):
        cddl = '''thing = {
          class: tstr, switch: tstr, let: tstr, var: tstr, protocol: tstr,
          associatedtype: tstr, some: tstr, any: tstr, actor: tstr,
          async: tstr, await: tstr, inout: tstr, repeat: tstr,
          self: tstr, Self: tstr, super: tstr, init: tstr, deinit: tstr,
          subscript: tstr, Type: tstr, Protocol: tstr, type: tstr,
          foo-bar: tstr, cborValue: tstr, schemaRule: tstr, depth: tstr,
          ? optional: null,
          nested: {default: bool},
          choice: "one" / "two"
        }
        text = tstr
        integer = -8..8
        number = 0..18446744073709551615
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
        let typed = try Thing.fromCBOR(bytes)
        precondition(typed.`class` == "ok")
        precondition(typed.`self_` == "ok")
        precondition(typed.`foo_bar` == "ok")
        precondition(typed.`cborValue_` == "ok")
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
