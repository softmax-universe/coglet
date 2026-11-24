"""Release inventory behavior; no network or invented producer binaries."""
import importlib.util
from pathlib import Path
import unittest
import tempfile
import subprocess
import sys
import json


def load():
    spec = importlib.util.spec_from_file_location('assemble', SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

SCRIPT = Path(__file__).with_name('assemble.py')


class InventoryTests(unittest.TestCase):
    def test_version_contract_and_complete_inventory(self):
        self.assertTrue(SCRIPT.is_file(), 'release assembler is missing')
        module = load()
        self.assertEqual(module.version('v0.0.0-rc1'), '0.0.0rc1')
        self.assertEqual(module.version('v1.2.3'), '1.2.3')
        for invalid in ['main', 'v1.2', 'v1.2.3/evil', 'v01.2.3', 'v1.2.3-rc0']:
            with self.assertRaises(ValueError):
                module.version(invalid)
        assets = module.expected('v0.0.0-rc1')
        self.assertEqual(len(assets), 10)
        self.assertEqual(len(set(assets)), len(assets))
        self.assertIn('coglet-v0.0.0-rc1-linux-armv7.tar.gz', assets)
        self.assertIn('coglet_surface-0.0.0rc1-py3-none-any.whl', assets)


    def test_source_bundle_uses_committed_bytes_and_runs_harness(self):
        module = load()
        self.assertTrue(hasattr(module, 'source_bundles'), 'source bundler missing')
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / 'repo'
            root.mkdir()
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root / 'Cargo.toml').write_text('[workspace]\n')
            (root / 'conformance').mkdir()
            (root / 'conformance/run').write_text('#!/bin/sh\nexit 0\n')
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=Test', '-c', 'user.email=test@example.org', 'commit', '-qm', 'fixture'], check=True)
            sha = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
            (root / 'Cargo.toml').write_text('UNCOMMITTED SECRET')
            out = Path(d) / 'out'
            module.source_bundles(root, out, 'v0.0.0-rc1', sha)
            import tarfile
            with tarfile.open(out / 'coglet-v0.0.0-rc1-rust-source.tar.gz') as archive:
                self.assertEqual(archive.extractfile('coglet-v0.0.0-rc1/Cargo.toml').read(), b'[workspace]\n')
            self.assertEqual(len(list(out.glob('*.tar.gz'))), 4)


    def test_binary_archive_requires_both_real_executables(self):
        module = load()
        self.assertTrue(hasattr(module, 'binary_bundle'), 'binary bundler missing')
        import shutil
        import tarfile
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            bins = root / 'bin'
            bins.mkdir()
            shutil.copy2(sys.executable, bins / 'cogletd')
            with self.assertRaises(FileNotFoundError):
                module.binary_bundle(bins, root / 'out', 'v0.0.0-rc1', 'linux-x86_64')
            (bins / 'coglet-hub').write_text('placeholder')
            with self.assertRaises(ValueError):
                module.binary_bundle(bins, root / 'out', 'v0.0.0-rc1', 'linux-x86_64')
            shutil.copy2(sys.executable, bins / 'coglet-hub')
            with self.assertRaises(ValueError):
                module.binary_bundle(bins, root / 'out', 'v0.0.0-rc1', 'linux-aarch64')
            module.binary_bundle(bins, root / 'out', 'v0.0.0-rc1', 'linux-x86_64')
            with tarfile.open(root / 'out/coglet-v0.0.0-rc1-linux-x86_64.tar.gz') as archive:
                self.assertEqual(set(archive.getnames()), {'cogletd', 'coglet-hub'})
                self.assertTrue(all(entry.mode & 0o111 for entry in archive))

    def test_inventory_refuses_missing_and_unexpected_files(self):
        module = load()
        self.assertTrue(hasattr(module, 'seal'), 'inventory sealer missing')
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            with self.assertRaises(ValueError):
                module.seal(root, 'v0.0.0-rc1', 'a' * 40)
            for name in module.expected('v0.0.0-rc1'):
                (root / name).write_bytes(b'unit inventory fixture')
            (root / 'extra').write_bytes(b'unreviewed asset')
            with self.assertRaises(ValueError):
                module.seal(root, 'v0.0.0-rc1', 'a' * 40)
            (root / 'extra').unlink()
            module.seal(root, 'v0.0.0-rc1', 'a' * 40)
            manifest = json.loads((root / 'release.json').read_text())
            self.assertEqual(manifest['commit'], 'a' * 40)
            self.assertEqual(len(manifest['assets']), 10)
            self.assertEqual(len((root / 'SHA256SUMS').read_text().splitlines()), 11)


    def test_cli_rejects_bad_tag_before_creating_outputs(self):
        result = subprocess.run([sys.executable, str(SCRIPT), 'sources', '--tag', 'evil', '--sha', 'a' * 40, '--out', '/nonexistent/release-test'], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('expected vMAJOR', result.stderr)


    def test_python_staging_sets_version_without_touching_source(self):
        module = load()
        self.assertTrue(hasattr(module, 'python_stage'), 'Python staging missing')
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / 'repo'
            package = root / 'sdk/surface-python'
            package.mkdir(parents=True)
            original = '[project]\nname = "coglet-surface"\nversion = "0.1.0"\n'
            (package / 'pyproject.toml').write_text(original)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=Test', '-c', 'user.email=test@example.org', 'commit', '-qm', 'fixture'], check=True)
            sha = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
            dest = Path(d) / 'stage'
            module.python_stage(root, dest, 'v0.0.0-rc1', sha)
            self.assertIn('version = "0.0.0rc1"', (dest / 'sdk/surface-python/pyproject.toml').read_text())
            self.assertEqual((package / 'pyproject.toml').read_text(), original)


if __name__ == '__main__':
    unittest.main()
