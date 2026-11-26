"""ADR-0035 independent Git oracle and version-boundary verification."""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from state_hash import snapshot_hash
from manage_oracle import tree, code_path
from test_common import SPEC, encode


class StateHash(unittest.TestCase):
    def test_state_hash_matches_git_sha256_tree(self):
        snapshots = [
            {}, {'/data/count': '30'},
            {'/code/bin/step.wasm': '0061736d01000000', '/data/a/nested': '00ff80', '/logs/run': ''},
            {'/data/foo/bar': '61', '/data/foo.bar': '62', '/data/foo0': '63'},
            {'/logs/café': 'ff00', '/logs/cafe\u0301': '80', '/logs/😀': '0a'},
        ]
        for files in snapshots:
            with self.subTest(files=files), tempfile.TemporaryDirectory() as td:
                subprocess.run(['git', 'init', '-q', '--object-format=sha256', td], check=True)
                for path, contents in files.items():
                    target = Path(td) / path[1:]
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(bytes.fromhex(contents))
                    target.chmod(0o644)
                subprocess.run(['git', '-C', td, '-c', 'core.autocrlf=false', 'add', '--all'], check=True)
                oid = subprocess.check_output(['git', '-C', td, 'write-tree'], text=True).strip()
                self.assertEqual(snapshot_hash(files), oid)
                self.assertEqual(snapshot_hash(dict(reversed(list(files.items())))), oid)

    def test_state_hash_rejects_unmappable_paths(self):
        for component in ['.git', '.Git', '.GIT', '.gIt']:
            for path in [f'/code/{component}/config', f'/data/{component}', f'/logs/a/{component}/x']:
                with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'unsafe path'):
                    snapshot_hash({path: ''})
            self.assertFalse(code_path('nested/' + component + '/config'))
        for files in [{'/data/a': '', '/data/a/b': ''}, {'/code/a/b': '', '/code/a': ''},
                      {'/logs/x/y/z': '00', '/logs/x': 'ff'}]:
            with self.subTest(files=files), self.assertRaisesRegex(ValueError, 'file_directory_collision'):
                snapshot_hash(files)
        # Similar names and Unicode normalization are not new forbidden names.
        snapshot_hash({'/data/.gitignore': '', '/data/.gitx': '', '/data/ＧＩＴ': ''})

    def test_code_tree_hash_is_restricted_snapshot_id(self):
        files = {'/code/bin/step.wasm': '0061736d01000000', '/data/a': '01', '/logs/a': 'ff'}
        restricted = {p: h for p, h in files.items() if p.startswith('/code/')}
        self.assertEqual(tree({p[len('/code/'):]: h for p, h in restricted.items()}), snapshot_hash(restricted))
        self.assertNotEqual(snapshot_hash(files), snapshot_hash(restricted))
        self.assertEqual(tree({}), snapshot_hash({}))
        # Root identity includes a code entry, not merely the code subtree.
        blob = hashlib.sha256(b'blob 1\0x').digest()
        subtree_body = b'100644 a\0' + blob
        subtree = hashlib.sha256(b'tree ' + str(len(subtree_body)).encode() + b'\0' + subtree_body).hexdigest()
        self.assertNotEqual(tree({'a': '78'}), subtree)

    def test_versioned_contracts_and_nonfilesystem_hashes(self):
        abi = json.loads((SPEC / 'messages/abi.schema.json').read_text())['$defs']
        db = json.loads((SPEC / 'messages/database.schema.json').read_text())['$defs']
        hub = json.loads((SPEC / 'messages/hub.schema.json').read_text())['$defs']
        old = json.loads((SPEC / 'frozen/hub-1.0/hub.schema.json.frozen').read_text())['$defs']
        self.assertEqual(abi['manifest']['properties']['version']['const'], '1.0')
        self.assertEqual(db['db-manifest']['properties']['version']['const'], '1.1')
        self.assertEqual(hub['hub-build-version']['enum'], ['2.0'])
        self.assertEqual(old['hub-build-version']['enum'], ['1.0'])
        self.assertIn('core-deterministic CBOR', (SPEC / 'frozen/hub-1.0/protocol.md').read_text())
        from test_abi import record_hash
        rec = {'version': '1.0', 'record_hash': 'ignored', 'data': 5}
        self.assertEqual(record_hash(rec), hashlib.sha256(encode({'version': '1.0', 'data': 5})).hexdigest())


if __name__ == '__main__':
    unittest.main()
