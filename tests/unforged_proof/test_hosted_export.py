"""Synthetic export-boundary tests; not hosted execution qualification."""
import copy
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore, Refused, TrustKey
from hosted_export import ExportAuthority, export_private


class HostedExportGuards(unittest.TestCase):
    def setUp(self):
        self.private = tempfile.TemporaryDirectory()
        self.addCleanup(self.private.cleanup)
        self.revoked = set()
        self.authority = ExportAuthority(ArtifactStore(self.private.name),
                                         TrustKey('operator-export', b'e'*32), self.revoked)
        self.scope = dict(sender='a'*64, recipient='b'*64, coglet='coglet:'+'c'*64,
                          objects=[dict(oid='d'*64, kind='blob', length=12)],
                          operations=['export', 'read'])
        self.calls = []

    def grant(self):
        return self.authority.issue(self.scope)

    def send(self, grant, scope=None, revoke=False):
        def serialize():
            self.calls.append('serialize')
            if revoke:
                self.revoked.add(grant)
            return b'private bytes'
        def write(value):
            self.calls.append('write')
            return value
        def upload(value):
            self.calls.append('upload')
            return value
        return export_private(self.authority, grant, scope or self.scope,
                              serialize=serialize, write=write, upload=upload)

    def test_explicit_export_and_read_grant_permits_exact_closure(self):
        self.assertEqual(self.send(self.grant()), b'private bytes')
        self.assertEqual(self.calls, ['serialize', 'write', 'upload'])

    def test_denial_precedes_serialization_object_writes_and_upload(self):
        grant = self.grant()
        changed = []
        for key in ('sender', 'recipient', 'coglet', 'objects', 'operations'):
            scope = copy.deepcopy(self.scope)
            scope[key] = {'sender':'f'*64, 'recipient':'f'*64,
                          'coglet':'coglet:'+'f'*64,
                          'objects':[dict(oid='e'*64, kind='blob', length=12)],
                          'operations':['export']}[key]
            changed.append(scope)
        for ref, scope in [(None, self.scope), ('0'*64, self.scope)]+[(grant,s) for s in changed]:
            with self.subTest(ref=ref, scope=scope), self.assertRaises(Refused):
                self.send(ref, scope)
            self.assertEqual(self.calls, [])
        self.revoked.add(grant)
        with self.assertRaises(Refused):
            self.send(grant)
        self.assertEqual(self.calls, [])

    def test_revocation_during_serialization_prevents_object_write_and_upload(self):
        with self.assertRaises(Refused):
            self.send(self.grant(), revoke=True)
        self.assertEqual(self.calls, ['serialize'])

    def test_modified_artifact_and_wrong_operator_key_refuse(self):
        grant = self.grant()
        other = ExportAuthority(self.authority.store, TrustKey('other', b'f'*32), set())
        with self.assertRaises(Refused):
            other.verify(grant, self.scope)
        path = Path(self.private.name) / grant
        path.chmod(0o600)
        path.write_bytes(path.read_bytes().replace(b'export', b'impost'))
        with self.assertRaises(Refused):
            self.send(grant)
        self.assertEqual(self.calls, [])

    def test_invalid_or_ambiguous_descriptor_refuses_before_issuance(self):
        changes = [dict(oid='d'*40, kind='blob', length=12),
                   dict(oid='d'*64, kind='blob', length=True),
                   dict(oid='d'*64, kind='blob', length=-1),
                   dict(oid='d'*64, kind='unknown', length=12),
                   dict(oid='d'*64, kind='blob', length=12, payload='private')]
        for obj in changes:
            scope = {**self.scope, 'objects':[obj]}
            with self.subTest(obj=obj), self.assertRaises(Refused):
                self.authority.issue(scope)
        for objects in ([], self.scope['objects'] * 2):
            with self.assertRaises(Refused):
                self.authority.issue({**self.scope, 'objects':objects})


if __name__ == '__main__':
    unittest.main()
