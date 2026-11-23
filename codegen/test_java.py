"""Java emitter regression tests, independent of optional Java toolchain."""
import importlib.util
from pathlib import Path
import unittest

class JavaEmitterTests(unittest.TestCase):
    def emitter(self):
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
}''')
            subprocess.run(['javac','--release','21','-d',tmp,str(path/'Generated.java'),str(path/'Codec.java')],check=True,capture_output=True,text=True)

    @unittest.skipUnless(__import__('shutil').which('mvn'), 'Maven not installed')
    def test_component_m_roundtrips_with_real_codec(self):
        import shutil
        import subprocess
        import tempfile
        from generate import parse
        root = Path(__file__).resolve().parents[1]
        # Separate projects let required-m runtime failures surface even when
        # optional-m generation fails javac before any tests can execute.
        for optional in (False, True):
            with self.subTest(optional=optional), tempfile.TemporaryDirectory(dir=root) as tmp:
                project = Path(tmp)
                shutil.copy(root/'sdk/surface-java/pom.xml', project/'pom.xml')
                package = project/'src/main/java/org/coglet/surface'
                package.mkdir(parents=True)
                shutil.copy(root/'sdk/surface-java/src/main/java/org/coglet/surface/Codec.java', package/'Codec.java')
                rules = parse('sample = {' + ('? ' if optional else '') + 'm: tstr}')
                for name, content in self.emitter().generate(rules).items():
                    target = project/Path(name).relative_to('sdk/surface-java')
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(content)
                test = project/'src/test/java/org/coglet/surface/CaptureTest.java'
                test.parent.mkdir(parents=True)
                constructor = 'new Generated.Presence<>("ok")' if optional else '"ok"'
                absent = '''
        var absent = new Generated.Sample(null);
        assertEquals(Map.of(), absent.toValue());
        byte[] empty = HexFormat.of().parseHex("a0");
        assertArrayEquals(empty, absent.toCbor());
        var decoded = Generated.Sample.fromCbor(empty);
        assertNull(decoded.m());
        assertArrayEquals(empty, decoded.toCbor());
''' if optional else ''
                test.write_text('''package org.coglet.surface;
import java.util.*;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;
class CaptureTest {
    @Test void preservesComponentAndWireBytes() {
        var sample = new Generated.Sample(''' + constructor + ''');
        assertEquals(Map.of("m", "ok"), sample.toValue());
        byte[] expected = HexFormat.of().parseHex("a1616d626f6b");
        assertArrayEquals(expected, sample.toCbor());
        var decodedPresent = Generated.Sample.fromCbor(expected);
        assertEquals(sample, decodedPresent);
        assertArrayEquals(expected, decodedPresent.toCbor());
''' + absent + '''
    }
}
''')
                result = subprocess.run(['mvn', '--batch-mode', '--no-transfer-progress', 'test'],
                                        cwd=project, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

if __name__ == '__main__': unittest.main()
