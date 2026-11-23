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
            (root / 'unrelated.rs').write_text('changed')
            self.assertEqual(initial, bundle.input_key(root, 'rust', 'cargo'))
            (root / 'Cargo.lock').write_text('new lock')
            self.assertNotEqual(initial, bundle.input_key(root, 'rust', 'cargo'))

    def test_restore_miss_falls_back_normally_but_fails_proof(self):
        spec = importlib.util.spec_from_file_location('bundle', Path(__file__).with_name('dependency_bundle.py'))
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        def missing(name, directory):
            raise FileNotFoundError('mirror unavailable')
        self.assertFalse(bundle.restore_one('missing', {}, missing, required=False))
        with self.assertRaisesRegex(RuntimeError, 'required dependency bundle'):
            bundle.restore_one('missing', {}, missing, required=True)


if __name__ == '__main__':
    unittest.main()
