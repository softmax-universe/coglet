"""Executed target regressions: schema identifiers cannot capture runtime helpers."""
import json
import keyword
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from cddl import parse
import generate
import emit_go
import emit_java
from test_java import jackson_classpath

ROOT = Path(__file__).resolve().parents[1]


def records(names):
    # Separate records exercise each spelling without intentional normalization collisions.
    return parse('\n'.join(f'sample{i} = {{{json.dumps(n)}: tstr, next: tstr}}'
                           for i, n in enumerate(names)))


def run(test, cmd, cwd, **kwargs):
    result = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, **kwargs)
    test.assertEqual(result.returncode, 0, result.stdout + result.stderr)


class IdentifierTests(unittest.TestCase):
    def test_python_helpers_import_and_typed_roundtrip(self):
        names = sorted(set(list(keyword.kwlist) + list(keyword.softkwlist) + [
            'field', 'dataclass', 'json', 'Message', 'Any', 'ClassVar', 'Literal',
            'str', 'list', 'dict', 'encode', 'decode', 'SCHEMA', 'RECORDS', 'self',
        ]) - {'_', 'to_cbor', 'from_cbor', 'next'})
        rules = records(names)
        rules.update(parse('optional = {? field: tstr, ? next: tstr}'))
        out = generate.generate(rules)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shutil.copytree(ROOT / 'sdk/surface-python/coglet_surface', root / 'coglet_surface')
            (root / 'coglet_surface/generated.py').write_text(out['sdk/surface-python/coglet_surface/generated.py'])
            (root / 'check.py').write_text('''import dataclasses
from coglet_surface import generated as g
from coglet_surface.codec import decode_profile, encode_profile
names = ''' + repr(names) + '''
for i, name in enumerate(names):
    cls = getattr(g, 'Sample' + str(i))
    member = next(f.name for f in dataclasses.fields(cls) if f.metadata['wire'] == name)
    value = cls(**{member: 'value', 'next': 'tail'})
    expected = {name: 'value', 'next': 'tail'}
    raw = value.to_cbor()
    assert raw == encode_profile(expected)
    assert decode_profile(raw) == expected
    assert cls.from_cbor(raw) == value
    assert cls.from_cbor(raw).to_cbor() == raw
for value in (g.Optional(), g.Optional(field='value'), g.Optional(field='value', next='tail')):
    raw = value.to_cbor()
    assert g.Optional.from_cbor(raw) == value
    assert g.Optional.from_cbor(raw).to_cbor() == raw
''')
            run(self, [sys.executable, 'check.py'], root)

    def test_rust_runtime_type_names_fail_closed(self):
        for name in ('result', 'serialize', 'deserialize'):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'reserved'):
                generate.generate(parse(f'{name} = tstr'))

    def test_java_runtime_members_fail_closed(self):
        for name in ('rule', 'toValue', 'toCbor', 'fromCbor', 'Codec', 'rest$'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                emit_java.generate(records([name]))

    def test_java_runtime_type_names_fail_closed(self):
        for name in ('Generated', 'Codec', 'Message', 'Presence', 'Object', 'String',
                     'Boolean', 'BigInteger', 'List', 'Map', 'LinkedHashMap', 'IllegalArgumentException'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                emit_java.generate(parse(f'{name} = {{value: tstr}}'))

    def test_go_runtime_members_and_symbols_fail_closed(self):
        for name in sorted(emit_go.RESERVED_MEMBERS):
            with self.subTest(field=name), self.assertRaises(ValueError):
                emit_go.generate(records([name]))
        for name in ('SchemaJSON', 'encode', 'decode', 'validate'):
            with self.subTest(rule=name), self.assertRaises(ValueError):
                emit_go.generate(parse(f'{name} = tstr'))
        with self.assertRaises(ValueError):
            emit_go.generate(parse('sample = {rest: tstr, * tstr => tstr}'))

    def test_python_message_methods_fail_closed(self):
        for name in ('to_cbor', 'from_cbor', '_rule', '_coglet_dataclasses'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                generate.generate(records([name]))

    @unittest.skipUnless(shutil.which('cargo'), 'Rust toolchain not available')
    def test_rust_helper_and_keyword_fields_roundtrip(self):
        names = sorted(generate.RUST_KEYWORDS | {'field', 'dataclass', 'next', 'encode', 'decode',
                       'serde', 'ciborium', 'Result', 'Serialize', 'Deserialize', 'Presence'})
        rules = records([n for n in names if n != 'next'])
        names = [n for n in names if n != 'next']
        out = generate.generate(rules)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shutil.copytree(ROOT / 'crates/coglet-types/src', root / 'src')
            manifest = (ROOT / 'crates/coglet-types/Cargo.toml').read_text()
            manifest = manifest.replace('version.workspace = true', 'version = "0.1.0"').replace('edition.workspace = true', 'edition = "2024"').replace('publish.workspace = true', 'publish = false')
            (root / 'Cargo.toml').write_text(manifest)
            (root / 'src/generated.rs').write_text(out['crates/coglet-types/src/generated.rs'])
            tests = ['#[test] fn helper_fields_roundtrip() {']
            for i, name in enumerate(names):
                ident = generate.identifier(name)
                tests += [f'let value = coglet_types::Sample{i} {{ {ident}: "value".into(), next: "tail".into() }};',
                          'let raw = value.to_cbor().unwrap();',
                          f'let back = coglet_types::Sample{i}::from_cbor(&raw).unwrap();',
                          f'assert_eq!(back.{ident}, "value");',
                          'assert_eq!(back.next, "tail");',
                          'assert_eq!(raw, back.to_cbor().unwrap());',
                          f'let expected = serde_json::json!({{{json.dumps(name)}: "value", "next": "tail"}});',
                          'let actual: serde_json::Value = ciborium::from_reader(raw.as_slice()).unwrap();',
                          'assert_eq!(actual, expected);']
            (root / 'tests').mkdir()
            (root / 'tests/identifiers.rs').write_text('\n'.join(tests + ['}']))
            run(self, ['cargo', 'test', '--quiet'], root)
            run(self, ['cargo', 'clippy', '--quiet', '--', '-D', 'warnings'], root)

    @unittest.skipUnless(shutil.which('go'), 'Go toolchain not available')
    def test_go_helper_and_keyword_fields_roundtrip(self):
        names = sorted(emit_go.GO_KEYWORDS | emit_go.RESERVED_SYMBOLS | {
            'field', 'dataclass', 'Message', 'Presence', 'asSample', 'isSample', 'm', 'raw', 'err', 'out'})
        rules = records(names)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shutil.copytree(ROOT / 'sdk/surface-go/codec', root / 'codec', ignore=shutil.ignore_patterns('*_test.go'))
            (root / 'go.mod').write_text('module github.com/softmax-universe/coglet/sdk/surface-go\n\ngo 1.23.0\n')
            (root / 'types').mkdir()
            (root / 'types/generated.go').write_text(emit_go.generate(rules)[emit_go.GO_TYPES_PATH])
            test = ['package types', 'import ("testing"; "bytes"; "reflect")', 'func TestIdentifiers(t *testing.T) {']
            for i, name in enumerate(names):
                ident = emit_go.exported(name, 'field')
                test += ['{', f'value := Sample{i}{{{ident}: "value", Next: "tail"}}',
                         'raw, err := value.ToCBOR(); if err != nil {t.Fatal(err)}',
                         f'back, err := DecodeSample{i}(raw); if err != nil {{t.Fatal(err)}}',
                         'if !reflect.DeepEqual(back, value) {t.Fatal(back)}',
                         'again, err := back.ToCBOR(); if err != nil || !bytes.Equal(raw, again) {t.Fatal(err)}',
                         f'if !reflect.DeepEqual(back.ToValue(), map[string]any{{{json.dumps(name)}:"value", "next":"tail"}}) {{t.Fatal(back)}}', '}']
            (root / 'types/identifiers_test.go').write_text('\n'.join(test + ['}']))
            run(self, ['go', 'test', './...'], root, env=dict(os.environ, GOFLAGS='-mod=mod'))

    @unittest.skipUnless(shutil.which('javac') and jackson_classpath(), 'JDK or Jackson jars not available')
    def test_java_helper_and_keyword_fields_roundtrip(self):
        names = sorted(emit_java.KEYWORDS | {'field', 'dataclass', 'nextValue', 'Presence', 'Message',
                       'encode', 'decode', 'm', 'e', 'raw', 'String', 'Map', 'List', 'Generated', 'registerAll'})
        out = emit_java.generate(records(names))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            pkg = root / 'org/coglet/surface'; pkg.mkdir(parents=True)
            (pkg / 'Generated.java').write_text(out['sdk/surface-java/src/main/java/org/coglet/surface/Generated.java'])
            (root / 'schema.json').write_text(out['sdk/surface-java/src/main/resources/schema.json'])
            shutil.copy(ROOT / 'sdk/surface-java/src/main/java/org/coglet/surface/Codec.java', pkg / 'Codec.java')
            test = ['package org.coglet.surface;', 'import java.util.*;', 'class Main { public static void main(String[] args) {']
            for i, name in enumerate(names):
                test += ['{', f'var value = new Generated.Sample{i}("value", "tail");',
                         'byte[] raw = value.toCbor();', f'var back = Generated.Sample{i}.fromCbor(raw);',
                         'if (!value.equals(back) || !Arrays.equals(raw, back.toCbor())) throw new AssertionError(back);',
                         f'if (!back.toValue().equals(Map.of({json.dumps(name)}, "value", "next", "tail"))) throw new AssertionError(back);', '}']
            (pkg / 'Main.java').write_text('\n'.join(test + ['}}']))
            cp = jackson_classpath()
            assert cp is not None
            run(self, ['javac', '--release', '21', '-cp', cp, '-d', str(root), *map(str, pkg.glob('*.java'))], root)
            run(self, ['java', '-cp', os.pathsep.join([str(root), cp]), 'org.coglet.surface.Main'], root)
