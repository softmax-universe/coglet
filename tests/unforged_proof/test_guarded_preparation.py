"""Initial preparation and staging tests, not hosted qualification."""
import copy
import gzip
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore, TrustKey, Refused
from hosted_export import ExportAuthority
from guarded_transport import RecipeAuthority, scope_for
from guarded_preparation import Sources, prepare

ROOT = Path(__file__).resolve().parents[2]

class PreparationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.parent = Path(self.tmp.name)
        self.revoked = set()
        store = ArtifactStore(self.parent/'authority')
        key = TrustKey('operator',b'x'*32)
        self.source_authority = ExportAuthority(store,key,self.revoked)
        self.recipe_authority = RecipeAuthority(store,key,self.revoked)
        trace = json.loads(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
        self.trace = trace
        self.sources = Sources(trace=json.dumps(trace).encode(),snapshot=json.dumps(trace['initial_files']).encode(),
            driver=b'\x7fELFtest',worker=b'worker',step=0,sender='a'*64,recipient='b'*64,
            coglet=trace['coglet'],image='native@sha256:'+'c'*64,salt='d'*64)
        self.scope = self.sources.scope()
        self.grant = self.source_authority.issue(self.scope)

    def prepared(self):
        return prepare(self.sources,self.source_authority,self.grant,self.scope)

    def recipe_grant(self, prepared):
        scope = scope_for(prepared.recipe,sender='a'*64,recipient='b'*64,coglet=self.trace['coglet'])
        return scope,self.recipe_authority.issue(scope)

    def test_initial_denial_precedes_decode_and_payload_serialization(self):
        self.revoked.add(self.grant)
        with patch('guarded_preparation.strict_json',side_effect=AssertionError('decoded')),patch('guarded_preparation.state_files',side_effect=AssertionError('serialized')):
            with self.assertRaises(Refused): self.prepared()
        self.assertFalse((self.parent/'stage').exists())

    def test_source_or_operator_binding_changes_refuse(self):
        for key,value in [('trace',b'{}'),('snapshot',b'{}'),('recipient','e'*64),('step',1),('salt','e'*64)]:
            values = self.sources.arguments()
            values[key] = value
            other = Sources(**values)
            with self.subTest(key=key),self.assertRaises(Refused):
                prepare(other,self.source_authority,self.grant,self.scope)

    def test_wrong_snapshot_refuses(self):
        values = self.sources.arguments()
        values['snapshot'] = b'{}'
        wrong = Sources(**values)
        scope = wrong.scope()
        with self.assertRaises(Refused): prepare(wrong,self.source_authority,self.source_authority.issue(scope),scope)

    def test_two_grants_stage_exact_bytes_and_create_strong_session(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope) as staged:
            self.assertTrue(staged.session.strong)
            self.assertEqual(staged.session.scope,scope)
            root = Path(staged.path)
            actual = {p.relative_to(root).as_posix():p.read_bytes() for p in root.rglob('*') if p.is_file()}
            self.assertEqual(actual,dict(prepared.files))
            self.assertEqual(os.stat(root).st_mode & 0o777,0o700)
        self.assertFalse((self.parent/'stage').exists())

    def test_missing_recipe_grant_creates_nothing(self):
        prepared = self.prepared()
        scope,_ = self.recipe_grant(prepared)
        with self.assertRaises(Refused):
            with prepared.stage(self.parent,'stage',self.recipe_authority,None,scope): pass
        self.assertFalse((self.parent/'stage').exists())

    def test_revocation_during_staging_stops_and_cleans_created_files(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        original = os.write
        writes = []
        def write(fd,raw):
            result = original(fd,raw)
            writes.append(result)
            self.revoked.add(self.grant)
            return result
        with patch('guarded_preparation.os.write',side_effect=write):
            with self.assertRaises(Refused):
                with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope): pass
        self.assertEqual(len(writes),1)
        self.assertFalse((self.parent/'stage').exists())

    def test_existing_root_and_symlink_parent_refuse(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        (self.parent/'stage').mkdir()
        with self.assertRaises(Refused):
            with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope): pass
        link = self.parent/'link'
        link.symlink_to(self.parent,target_is_directory=True)
        with self.assertRaises(Refused):
            with prepared.stage(link,'other',self.recipe_authority,grant,scope): pass
        self.assertFalse((self.parent/'other').exists())

    def test_nonobject_trace_refuses(self):
        for raw in (b'[]',b'null',b'3',b'"x"'):
            values = self.sources.arguments()
            values['trace'] = raw
            source = Sources(**values)
            scope = source.scope()
            with self.subTest(raw=raw),self.assertRaises(Refused):
                prepare(source,self.source_authority,self.source_authority.issue(scope),scope)

    def test_body_oserror_is_preserved(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        marker = OSError('body marker')
        with self.assertRaises(OSError) as caught:
            with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope):
                raise marker
        self.assertIs(caught.exception,marker)
        self.assertFalse((self.parent/'stage').exists())

    def test_cleanup_fault_closes_all_fds_and_retries(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        before = len(list(Path('/proc/self/fd').iterdir()))
        original = os.unlink
        calls = []
        def unlink(*args,**kwargs):
            calls.append(1)
            if len(calls) == 1: raise PermissionError('injected cleanup failure')
            return original(*args,**kwargs)
        with patch('guarded_preparation.os.unlink',side_effect=unlink):
            with self.assertRaises(Refused):
                with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope): pass
        self.assertEqual(len(list(Path('/proc/self/fd').iterdir())),before)
        self.assertFalse((self.parent/'stage').exists())

    def test_cleanup_removes_unexpected_files_without_following_symlinks(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        outside = self.parent/'outside'
        outside.write_bytes(b'keep')
        before = len(list(Path('/proc/self/fd').iterdir()))
        with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope) as staged:
            root = Path(staged.path)
            (root/'unexpected').write_bytes(b'private')
            (root/'escape').symlink_to(outside)
        self.assertEqual(outside.read_bytes(),b'keep')
        self.assertFalse((self.parent/'stage').exists())
        self.assertEqual(len(list(Path('/proc/self/fd').iterdir())),before)

    def test_recipe_revocation_stops_next_write(self):
        prepared = self.prepared()
        scope,grant = self.recipe_grant(prepared)
        original = os.write
        writes = []
        def write(fd,raw):
            count = original(fd,raw)
            writes.append(count)
            self.revoked.add(grant)
            return count
        with patch('guarded_preparation.os.write',side_effect=write),self.assertRaises(Refused):
            with prepared.stage(self.parent,'stage',self.recipe_authority,grant,scope): pass
        self.assertEqual(len(writes),1)
        self.assertFalse((self.parent/'stage').exists())

    def test_retained_data_and_scope_cannot_be_mutated_through_views(self):
        prepared = self.prepared()
        with self.assertRaises(TypeError): prepared.files['evil'] = b'x'
        recipe = prepared.recipe
        recipe['image'] = 'changed'
        self.assertNotEqual(prepared.recipe['image'],'changed')
        values = self.sources.arguments()
        values['trace'] = bytearray(values['trace'])
        with self.assertRaises(Refused): Sources(**values)

if __name__ == '__main__': unittest.main()
