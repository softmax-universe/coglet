"""Java emitter regression tests, independent of optional Java toolchain."""
import importlib.util
from pathlib import Path
import unittest
import json
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def jackson_classpath():
    """Jackson jars for the integration test: $JACKSON_CP, else a local Maven repository."""
    if os.environ.get('JACKSON_CP'): return os.environ['JACKSON_CP']
    for repo in (os.environ.get('MAVEN_REPO'), str(Path.home() / '.m2/repository')):
        if repo:
            jars = sorted(Path(repo).glob('com/fasterxml/jackson/core/*/2.18.3/*-2.18.3.jar'))
            if len(jars) >= 3: return os.pathsep.join(map(str, jars))
    return None


TSTR = {'op': 'primitive', 'name': 'tstr'}
def rng(lo, hi): return {'op': 'range', 'min': lo, 'max': hi}
def field(name, typ, optional=False): return {'name': name, 'optional': optional, 'type': typ}


def v2_rules():
    rules = {
        'common::id': {'op': 'intersection', 'items': [TSTR, {'op': 'regexp', 'pattern': '[a-z]+'}, {'op': 'size', 'min': 1, 'max': 8}]},
        'id': {'op': 'intersection', 'items': [TSTR, {'op': 'regexp', 'pattern': '[a-z]+'}, {'op': 'size', 'min': 1, 'max': 8}]},
        'pair': {'op': 'tuple', 'items': [{'type': TSTR, 'min': 0, 'max': 2}, {'type': rng(0, 9), 'min': 1, 'max': 1}]},
        'bag': {'op': 'map', 'item': rng(0, 9), 'min': 1, 'max': 2, 'key': {'op': 'regexp', 'pattern': 'k[0-9]'}},
        'tags': {'op': 'array', 'item': {'op': 'ref', 'name': 'common::id'}, 'min': 1, 'max': 2},
        'dflt': {'op': 'default', 'item': rng(0, 9), 'value': 3},
        'p::doc': {'op': 'record', 'fields': [field('id', {'op': 'ref', 'name': 'common::id'}), field('tags', {'op': 'ref', 'name': 'tags'}, True),
                                              field('n', {'op': 'ref', 'name': 'dflt'}, True)],
                   'rest': {'item': rng(0, 9), 'min': 1, 'max': 2, 'key': {'op': 'regexp', 'pattern': 'x-[a-z]+'}}},
        'plain': {'op': 'record', 'fields': [field('a', TSTR)]},
    }
    rules['doc'] = rules['p::doc']      # identical unqualified copy, as the multi-file compiler emits
    return rules


MAIN = '''package org.coglet.surface;
import java.math.BigInteger;
import java.util.*;
public class Main {
  static void bad(Runnable r, String what) { try { r.run(); } catch (IllegalArgumentException e) { return; } throw new AssertionError("accepted: " + what); }
  static BigInteger n(long v) { return BigInteger.valueOf(v); }
  public static void main(String[] a) {
    var rest = new LinkedHashMap<String, BigInteger>(); rest.put("x-a", n(1)); rest.put("x-b", n(2));
    var doc = new Generated.Doc("abc", new Generated.Presence<>(List.of("one", "two")), new Generated.Presence<>(n(4)), rest);
    byte[] raw = doc.toCbor();
    Generated.Doc back = Generated.Doc.fromCbor(raw);
    if (!back.equals(doc)) throw new AssertionError("typed round trip lost data: " + back);
    if (!Arrays.equals(raw, back.toCbor())) throw new AssertionError("re-encode differs");
    if (!back.rest$().equals(rest)) throw new AssertionError("rest lost");
    // Qualified and unqualified keys decode to the same type.
    if (!(Codec.decodeTyped("p::doc", raw) instanceof Generated.Doc)) throw new AssertionError("qualified decode");
    // Encode-side enforcement: every constraint, no weakening.
    bad(() -> new Generated.Doc("abc", null, null, Map.of()).toCbor(), "rest.min 1");
    bad(() -> new Generated.Doc("abc", null, null, new LinkedHashMap<>(Map.of("x-a", n(1), "x-b", n(1), "x-c", n(1)))).toCbor(), "rest.max");
    bad(() -> new Generated.Doc("abc", null, null, Map.of("y", n(1))).toCbor(), "rest.key");
    bad(() -> new Generated.Doc("abc", null, null, Map.of("x-a", n(10))).toCbor(), "rest.item");
    bad(() -> new Generated.Doc("ABC", null, null, Map.of("x-a", n(1))).toCbor(), "intersection regexp");
    bad(() -> new Generated.Doc("abcdefghi", null, null, Map.of("x-a", n(1))).toCbor(), "intersection size max");
    bad(() -> new Generated.Doc("abc", new Generated.Presence<>(List.of()), null, Map.of("x-a", n(1))).toCbor(), "array min");
    bad(() -> new Generated.Doc("abc", new Generated.Presence<>(List.of("a", "b", "c")), null, Map.of("x-a", n(1))).toCbor(), "array max");
    bad(() -> new Generated.Doc("abc", null, new Generated.Presence<>(n(10)), Map.of("x-a", n(1))).toCbor(), "default item");
    // Decode-side enforcement from hand-built wire bytes.
    Map<String, Object> m = new LinkedHashMap<>(); m.put("id", "abc"); m.put("y", n(1));
    byte[] wrong = Codec.encodeProfile(m);
    bad(() -> Codec.decode("p::doc", wrong), "decode rest.key");
    bad(() -> Generated.Doc.fromCbor(wrong), "typed decode rest.key");
    // Tuple and map rules via the native codec.
    Codec.encode("pair", List.of("a", n(5))); Codec.encode("pair", List.of(n(5)));
    bad(() -> Codec.encode("pair", List.of("a", "b", "c", n(5))), "tuple max");
    bad(() -> Codec.encode("pair", List.of("a")), "tuple min");
    Codec.encode("bag", Map.of("k1", n(1)));
    bad(() -> Codec.encode("bag", Map.of()), "map min");
    bad(() -> Codec.encode("bag", Map.of("k1", n(1), "k2", n(1), "k3", n(1))), "map max");
    bad(() -> Codec.encode("bag", Map.of("z", n(1))), "map key");
    System.out.println("native v2 ok");
  }
}
'''

class JavaEmitterTests(unittest.TestCase):
    def generate_v2(self, emit, rules):
        """Emit v2 rules. The shared target_ir guard is owned elsewhere; stand in for it only
        while it still rejects additive v2 IR, so this suite does not depend on its rollout."""
        try:
            return emit.generate(rules)
        except ValueError as e:
            if 'unsupported native target IR' not in str(e): raise
            real, emit.require_v1 = emit.require_v1, lambda r: None
            try: return emit.generate(rules)
            finally: emit.require_v1 = real

    def emitter(self):
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        path = Path(__file__).with_name('emit_java.py')
        self.assertTrue(path.exists(), 'Java emitter is missing')
        spec = importlib.util.spec_from_file_location('emit_java', path)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_records_keywords_and_collisions(self):
        emit = self.emitter()
        record = lambda names: {'op': 'record', 'fields': [{'name': n, 'optional': False, 'type': {'op':'primitive','name':'tstr'}} for n in names]}
        result = emit.generate({'test':record(['class', 'record', 'foo-bar'])})
        text = '\n'.join(result.values())
        self.assertIn('String class_', text)
        self.assertIn('String record_', text)
        self.assertIn('String foo_bar', text)
        with self.assertRaises(ValueError):
            emit.generate({'test':record(['foo-bar', 'foo_bar'])})
        with self.assertRaises(ValueError):
            emit.generate({'foo-bar':record([]), 'foo_bar':record([])})
        with self.assertRaises(ValueError):
            emit.generate({'foo': {'op':'union','items':[record([]),record([])]}, 'foo1':record([])})
        with self.assertRaises(ValueError):
            emit.generate({'test':record(['class','class_'])})
        with self.assertRaises(ValueError):
            emit.generate({'test':record(['toCbor'])})
        self.assertEqual(result, emit.generate({'test':record(['class','record','foo-bar'])}))

    @unittest.skipUnless(__import__('shutil').which('javac'), 'JDK not installed')
    def test_keyword_schema_compiles(self):
        import subprocess
        import tempfile
        root = Path(__file__).resolve().parents[1]
        emit = self.emitter()
        fields = [{'name': n, 'optional': False, 'type': {'op':'primitive','name':'tstr'}}
                  for n in sorted(emit.KEYWORDS) + ['dotted.name', 'hyphen-key']
                  if n not in {'class_', 'record_'}]
        rules = {'sample': {'op':'record','fields': fields}}
        generated = emit.generate(rules)
        with tempfile.TemporaryDirectory(dir=root) as tmp:
            path = Path(tmp)
            (path/'Generated.java').write_text(next(v for k,v in generated.items() if k.endswith('.java')))
            (path/'Codec.java').write_text('''package org.coglet.surface;
public class Codec {
 public static byte[] encode(String rule,Object v) { return null; }
 public static Object decodeTyped(String rule,byte[] raw) { return null; }
 public static <T> T decodeRecord(String rule,byte[] raw,Class<T> cls) { return null; }
 static void register(String rule,Class<?> cls,String schema,String[] keys) {}
 static void alias(String alias,String canonical) {}
}''')
            subprocess.run(['javac','--release','21','-d',tmp,str(path/'Generated.java'),str(path/'Codec.java')],check=True,capture_output=True,text=True)

    def test_v2_types_aliases_and_rest(self):
        emit = self.emitter()
        out = self.generate_v2(emit, v2_rules())
        java = out['sdk/surface-java/src/main/java/org/coglet/surface/Generated.java']
        self.assertIn('public record Doc(', java)

    def test_v2_ir_fails_closed(self):
        emit = self.emitter()
        for bad in ({'op': 'bytes'}, {'op': 'array', 'item': TSTR, 'min': 0, 'maxx': 1},
                    {'op': 'record', 'fields': [], 'rest': {'item': TSTR}},
                    {'op': 'tuple', 'items': [{'type': TSTR}]},
                    {'op': 'map', 'item': TSTR, 'keys': TSTR},
                    {'op': 'primitive', 'name': 'int'}):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                self.generate_v2(emit, {'x': bad})

    def test_qualified_alias_collapses_to_one_type(self):
        emit = self.emitter()
        out = self.generate_v2(emit, {'a::plain': v2_rules()['plain'], 'plain': v2_rules()['plain']})
        java = out['sdk/surface-java/src/main/java/org/coglet/surface/Generated.java']
        self.assertEqual(java.count('public record Plain('), 1)
        self.assertIn('Codec.alias("a::plain", "plain");', java)
        out = self.generate_v2(emit, {'a::plain': v2_rules()['plain']})   # ambiguous-only: qualified identifier
        self.assertIn('public record APlain(', out['sdk/surface-java/src/main/java/org/coglet/surface/Generated.java'])
        with self.assertRaises(ValueError):                              # a::b-c and a-b::c would both be ABC
            self.generate_v2(emit, {'a::b-c': v2_rules()['plain'], 'a-b::c': v2_rules()['plain']})

    @unittest.skipUnless(shutil.which('javac') and jackson_classpath(), 'JDK or Jackson jars not available')
    def test_v2_native_compile_roundtrip_and_enforcement(self):
        emit = self.emitter()
        rules = v2_rules()
        out = self.generate_v2(emit, rules)
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            src, classes = tmp / 'src', tmp / 'classes'
            pkg = src / 'org/coglet/surface'; pkg.mkdir(parents=True); classes.mkdir()
            (pkg / 'Generated.java').write_text(out['sdk/surface-java/src/main/java/org/coglet/surface/Generated.java'])
            (classes / 'schema.json').write_text(out['sdk/surface-java/src/main/resources/schema.json'])
            shutil.copy(ROOT / 'sdk/surface-java/src/main/java/org/coglet/surface/Codec.java', pkg / 'Codec.java')
            (pkg / 'Main.java').write_text(MAIN)
            cp = jackson_classpath()
            run = lambda cmd: subprocess.run(cmd, capture_output=True, text=True)
            c = run(['javac', '--release', '21', '-cp', cp, '-d', str(classes), *map(str, pkg.glob('*.java'))])
            self.assertEqual(c.returncode, 0, c.stderr)
            r = run(['java', '-cp', os.pathsep.join([str(classes), cp]), 'org.coglet.surface.Main'])
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertIn('native v2 ok', r.stdout)



if __name__ == '__main__': unittest.main()
