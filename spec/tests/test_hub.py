"""Hub schema/golden validation, not runtime behavioral conformance."""
import copy
import hashlib
import json
from pathlib import Path
import struct
import unittest

import cbor2
from jsonschema import Draft202012Validator
from pycddl import Schema, ValidationError
from referencing import Registry, Resource

from test_common import SPEC, decode, encode


def read_frame(raw):
    """Single-frame oracle; operation ordering is tested by the wire harness."""
    if len(raw) < 4:
        raise ValueError('short prefix')
    size = struct.unpack('>I', raw[:4])[0]
    if not 1 <= size <= 1048576 or len(raw) != size + 4:
        raise ValueError('length')
    return decode(raw[4:])


class HubFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = json.loads((SPEC / 'messages/hub.schema.json').read_text())
        common = json.loads((SPEC / 'messages/common.schema.json').read_text())
        cls.registry = Registry().with_resource(
            'https://coglet.dev/spec/messages/common.schema.json', Resource.from_contents(common))
        cls.cddl = (SPEC / 'messages/hub.cddl').read_text() + (SPEC / 'messages/common.cddl').read_text()
        cls.cases = json.loads((SPEC / 'fixtures/hub/cases.json').read_text())

    def test_hub_dual_schema_and_golden(self):
        Draft202012Validator.check_schema(self.root)
        self.assertEqual(len(self.cases), len({c['name'] for c in self.cases}))
        covered = set()
        for case in self.cases:
            with self.subTest(case=case['name']):
                rule = case['type']
                root = dict(self.root, **{'$ref': f'#/$defs/{rule}'})
                self.assertEqual(Draft202012Validator(root, registry=self.registry).is_valid(case['value']), case['valid'])
                validator = Schema(f'fixture = {rule}\n' + self.cddl)
                try:
                    validator.validate_cbor(encode(case['value']))
                    valid = True
                except ValidationError:
                    valid = False
                self.assertEqual(valid, case['valid'], 'CDDL')
                if case['valid']:
                    covered.add(rule)
                    raw = bytes.fromhex(case['cbor_hex'])
                    self.assertEqual(encode(case['value']), raw)
                    self.assertEqual(decode(raw), case['value'])
                    validator.validate_cbor(raw)
                    self.assertEqual(read_frame(struct.pack('>I', len(raw)) + raw), case['value'])
        self.assertEqual(covered, set(self.root['$defs']))
        print(f'hub: {len(self.cases)} dual-schema cases; {len(covered)} golden rule vectors')

    def test_frame_rejections(self):
        cases = json.loads((SPEC / 'fixtures/hub/framing.json').read_text())
        self.assertTrue(cases)
        for case in cases:
            with self.subTest(case=case['name']):
                with self.assertRaises((ValueError, cbor2.CBORDecodeError, UnicodeError)):
                    read_frame(bytes.fromhex(case['hex']))

    def test_case_data_is_wire_shaped(self):
        cases = [json.loads(p.read_text()) for p in sorted((SPEC.parent / 'conformance/hub').glob('*.json'))]
        self.assertTrue(cases)
        self.assertEqual(len(cases), len({c['name'] for c in cases}))
        request = dict(self.root, **{'$ref': '#/$defs/hub-frame'})
        for case in cases:
            self.assertTrue(case['steps'])
            self.assertTrue(case['name'])
            self.assertIn(case['peer_role'], ['hub', 'device'])
            self.assertIn('hub.' + case['name'], case['fixtures'])
            if case['peer_role'] == 'hub':
                self.assertIn('hub.reverse-stream', case['fixtures'])
            for step in case['steps']:
                self.assertEqual(len(set(step) & {'send', 'expect', 'match', 'journal_hash'}), 1)
                if 'send' in step:
                    Draft202012Validator(request, registry=self.registry).validate(step['send'])
                    frame = step['send']
                    if 'type' in frame:
                        self.assertEqual(step['stream'], int(frame['correlation_id'], 16))
                        self.assertEqual(step['finish'], frame['type'] == 'end')
                else:
                    self.assertGreater(step.get('within_ms', 200), 0)

    def test_signing_vectors(self):
        # openssl is available on the supported CI Linux fixture runner.
        import subprocess
        import tempfile
        vectors = json.loads((SPEC / 'fixtures/hub/signatures.json').read_text())
        self.assertEqual({v['statement']['purpose'] for v in vectors},
                         {'delegation', 'hub-ref', 'published-ref', 'logs-ref'})
        for vector in vectors:
            payload = ('coglet/hub/0:' + vector['statement']['purpose'] + '\n').encode() + encode(vector['statement'])
            self.assertEqual(payload.hex(), vector['preimage_hex'])
            with tempfile.TemporaryDirectory() as td:
                path = Path(td)
                (path / 'key.der').write_bytes(bytes.fromhex('302a300506032b6570032100' + vector['signer']))
                (path / 'sig').write_bytes(bytes.fromhex(vector['signature']))
                for altered in [False, True]:
                    (path / 'msg').write_bytes(payload + (b'X' if altered else b''))
                    run = subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER',
                        '-inkey', str(path / 'key.der'), '-rawin', '-in', str(path / 'msg'),
                        '-sigfile', str(path / 'sig')], capture_output=True)
                    self.assertEqual(run.returncode == 0, not altered)

    def test_real_git_bundle_and_archive_digest(self):
        import subprocess
        import tempfile
        fixture = json.loads((SPEC / 'fixtures/hub/git.json').read_text())
        bundle = bytes.fromhex(fixture['bundle_hex'])
        self.assertEqual(len(bundle), fixture['size'])
        self.assertEqual(hashlib.sha256(bundle).hexdigest(), fixture['sha256'])
        archive = bytes.fromhex(fixture['archive_hex'])
        statement = fixture['update']['statement']
        self.assertEqual(hashlib.sha256(archive).hexdigest(), statement['archive_sha256'])
        with tempfile.TemporaryDirectory() as td:
            path = Path(td)
            subprocess.run(['git', 'init', '-q', str(path / 'repo')], check=True)
            (path / 'bundle').write_bytes(bundle)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(path / 'repo'), *args], stderr=subprocess.STDOUT)
            git('bundle', 'verify', str(path / 'bundle'))
            git('fetch', '-q', str(path / 'bundle'), 'refs/heads/hub:refs/heads/hub')
            git('fsck', '--strict')
            self.assertEqual(git('archive', '--format=tar', statement['new']), archive)
            paths = git('ls-tree', '-r', '--name-only', statement['new']).decode().splitlines()
            self.assertIn('bin/step.wasm', paths)
            self.assertFalse(any(p.startswith('data/') for p in paths))
            signed = fixture['update']
            (path / 'key.der').write_bytes(bytes.fromhex('302a300506032b6570032100' + signed['signer']))
            (path / 'sig').write_bytes(bytes.fromhex(signed['signature']))
            (path / 'msg').write_bytes(b'coglet/hub/0:hub-ref\n' + encode(statement))
            subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER',
                '-inkey', str(path / 'key.der'), '-rawin', '-in', str(path / 'msg'),
                '-sigfile', str(path / 'sig')], check=True, capture_output=True)


if __name__ == '__main__':
    unittest.main()
