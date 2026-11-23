import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GeneratorTests(unittest.TestCase):
    def test_parse_common_and_refuse_unsupported(self):
        self.assertTrue((ROOT / 'codegen/generate.py').exists(), 'missing CDDL generator')
        spec = importlib.util.spec_from_file_location('generate', ROOT / 'codegen/generate.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        rules = module.parse((ROOT / 'spec/messages/common.cddl').read_text())
        self.assertEqual(rules['uint64'], {'op': 'range', 'min': 0, 'max': 18446744073709551615})
        self.assertEqual(rules['event']['fields'][0]['name'], 'source')
        with self.assertRaises(ValueError):
            module.parse('bad = bstr .cbor any')
    def test_map_group_choices_lower_to_record_union(self):
        spec = importlib.util.spec_from_file_location('generate', ROOT / 'codegen/generate.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        group = 'x = { (op: "a", value: tstr) // (op: "b", ? count: 0..10) }'
        union = 'x = {op: "a", value: tstr} / {op: "b", ? count: 0..10}'
        self.assertEqual(module.parse(group), module.parse(union))
        for bad in ['x = { (op: "a") / (op: "b") }',
                    'x = { (op: "a") // }', 'x = { (op: "a"), common: tstr }']:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                module.parse(bad)

    def test_check_detects_stale_and_is_read_only(self):
        import shutil
        import subprocess
        import sys
        import tempfile
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            root = Path(temp)
            (root / 'codegen').mkdir()
            shutil.copy(ROOT / 'codegen/generate.py', root / 'codegen/generate.py')
            (root / 'spec/messages').mkdir(parents=True)
            (root / 'spec/messages/test.cddl').write_text('thing = {name: tstr}')
            cmd = [sys.executable, str(root / 'codegen/generate.py')]
            subprocess.run(cmd, check=True, capture_output=True)
            subprocess.run(cmd + ['--check'], check=True, capture_output=True)
            out = root / 'sdk/surface-python/coglet_surface/generated.py'
            out.write_text(out.read_text() + '# stale\n')
            result = subprocess.run(cmd + ['--check'], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('stale generated files', result.stderr)
            self.assertTrue(out.read_text().endswith('# stale\n'))
            subprocess.run(cmd, check=True, capture_output=True)
            subprocess.run(cmd + ['--check'], check=True, capture_output=True)


if __name__ == '__main__':
    unittest.main()
