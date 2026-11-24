"""Dependency mirror boundary tests; stdlib only."""
import importlib.util
import tempfile
import unittest
from pathlib import Path


class BundleTests(unittest.TestCase):
    def test_roundtrip_and_corruption_is_atomic(self):
        script = Path(__file__).with_name('dependency_bundle.py')
        self.assertTrue(script.exists(), 'dependency bundle implementation is missing')
        spec = importlib.util.spec_from_file_location('bundle', script)
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            source = base / 'source'
            source.mkdir()
            (source / 'jar').write_bytes(b'real fixture bytes')
            archive = base / 'bundle.tar.gz'
            bundle.pack({'repository': source}, archive)
            dest = base / 'dest'
            bundle.unpack(archive, {'repository': dest}, bundle.digest(archive))
            self.assertEqual((dest / 'jar').read_bytes(), b'real fixture bytes')
            (dest / 'jar').write_bytes(b'untouched')
            with self.assertRaisesRegex(ValueError, 'digest'):
                bundle.unpack(archive, {'repository': dest}, '0' * 64)
            self.assertEqual((dest / 'jar').read_bytes(), b'untouched')

    def test_rejects_traversal_links_and_unknown_roots_before_install(self):
        import io
        import tarfile
        spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('dependency_bundle.py'))
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        for name, link in [('repository/../../escape', None), ('repository/link', '../../escape'),
                           ('repository/link', '../other/file'), ('unknown/file', None)]:
            with self.subTest(name=name, link=link), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                archive = base / 'bad.tar.gz'
                with tarfile.open(archive, 'w:gz') as stream:
                    good = tarfile.TarInfo('repository/good')
                    good.size = 1
                    stream.addfile(good, io.BytesIO(b'x'))
                    entry = tarfile.TarInfo(name)
                    if link:
                        entry.type = tarfile.SYMTYPE
                        entry.linkname = link
                    stream.addfile(entry)
                dest = base / 'dest'
                with self.assertRaises((ValueError, tarfile.TarError)):
                    bundle.unpack(archive, {'repository': dest}, bundle.digest(archive))
                self.assertFalse(dest.exists())

    def test_keys_follow_dependency_inputs_not_unrelated_source(self):
        spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('dependency_bundle.py'))
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.lock').write_text('locked')
            initial = bundle.input_key(root, 'rust', 'cargo')
            from unittest.mock import patch
            with patch.dict(bundle.os.environ, {'ImageVersion': 'new-runner-image'}):
                self.assertEqual(initial, bundle.input_key(root, 'rust', 'cargo'))
            (root / 'pyproject.toml').write_text('[project]\nname = "x"\n')
            python = bundle.input_key(root, 'rust', 'python')
            self.assertEqual(python, bundle.input_key(root, 'rust', 'python'))
            gems = bundle.input_key(root, 'conformance', 'gems')
            rustup = bundle.input_key(root, 'rust', 'rustup')
            (root / 'unrelated.rs').write_text('changed')
            self.assertEqual(initial, bundle.input_key(root, 'rust', 'cargo'))
            self.assertEqual(python, bundle.input_key(root, 'rust', 'python'))
            self.assertEqual(gems, bundle.input_key(root, 'conformance', 'gems'))
            self.assertEqual(rustup, bundle.input_key(root, 'rust', 'rustup'))
            (root / 'Cargo.lock').write_text('new lock')
            self.assertNotEqual(initial, bundle.input_key(root, 'rust', 'cargo'))
            self.assertEqual(python, bundle.input_key(root, 'rust', 'python'))
            (root / 'pyproject.toml').write_text('[project]\nname = "y"\n')
            self.assertNotEqual(python, bundle.input_key(root, 'rust', 'python'))
            (root / 'Gemfile.lock').write_text('GEM\n')
            self.assertNotEqual(gems, bundle.input_key(root, 'conformance', 'gems'))
            (root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel = "1.100.0"\n')
            self.assertNotEqual(rustup, bundle.input_key(root, 'rust', 'rustup'))

    def test_restore_miss_falls_back_normally_but_fails_proof(self):
        spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('dependency_bundle.py'))
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        def missing(name, directory):
            raise FileNotFoundError('mirror unavailable')
        self.assertFalse(bundle.restore_one('missing', {}, missing, required=False))
        def timeout(name, directory):
            import subprocess
            raise subprocess.TimeoutExpired('gh', 180)
        self.assertFalse(bundle.restore_one('slow', {}, timeout, required=False))
        with self.assertRaisesRegex(RuntimeError, 'required dependency bundle'):
            bundle.restore_one('missing', {}, missing, required=True)

    def test_toolchain_miss_installs_declared_components_and_wasi_target(self):
        from unittest.mock import patch
        spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('dependency_bundle.py'))
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        with patch.object(bundle.subprocess, 'run') as run:
            bundle.install_toolchain_on_miss('1.100.0', restored=True)
            run.assert_not_called()
            bundle.install_toolchain_on_miss('1.100.0', restored=False)
            run.assert_called_once_with(['rustup', 'toolchain', 'install', '1.100.0', '--profile',
                                         'minimal', '--component', 'clippy,rustfmt', '--target',
                                         'wasm32-wasip1'], check=True)


if __name__ == '__main__':
    unittest.main()
