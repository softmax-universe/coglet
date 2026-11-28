"""Local input/tree controls, not hosted qualification."""
import hashlib
import unittest
from standard_step import Refused, state_files, snapshot_tree

class StandardStep(unittest.TestCase):
    def test_input_has_only_prior_state_and_current_payload(self):
        files = state_files({'/code/a':'00','/data/x':'6162'}, {'prefix':{},'input':{}})
        self.assertEqual(files['code/a'], b'\0')
        self.assertEqual(set(files), {'code/a','data/x','proof-input.json'})

    def test_unmappable_snapshot_refuses_before_staging(self):
        for snapshot in ({'/../x':'00'},{'/a/.git/b':'00'},{'/a':'00','/a/b':'00'},
                         {'/proof-input.json':'00'},{'/a':'zz'},{'relative':'00'}, {'/a//b':'00'}):
            with self.subTest(snapshot=snapshot), self.assertRaises(Refused):
                state_files(snapshot, {})

    def test_sha256_tree_identity_includes_complete_paths_and_bytes(self):
        self.assertEqual(snapshot_tree({}), hashlib.sha256(b'tree 0\0').hexdigest())
        a = snapshot_tree({'a':b'hello','b/x':b'world'})
        for changed in ({'a':b'hello'}, {'a':b'Hello','b/x':b'world'}, {'a':b'hello','c/x':b'world'}):
            self.assertNotEqual(a, snapshot_tree(changed))

    def test_missing_or_false_miss_confirmation_refuses(self):
        from standard_step import require_fresh
        baseline = dict(request='a'*40, cache_hit=False,worker_image='image',server_confirmed_nocache=True)
        require_fresh(baseline,request='a'*40,image='image')
        for flag in (None,False,0,'true'):
            value = dict(baseline,server_confirmed_nocache=flag)
            with self.subTest(flag=flag), self.assertRaises(Refused):
                require_fresh(value,request='a'*40,image='image')

    def test_wire_tree_requires_regular_modes_and_complete_unique_paths(self):
        for rows in ([{'path':'a','mode':'100755','hex':'00'}],
                     [{'path':'a','mode':'100644','hex':'00'}]*2,
                     [{'path':'a','mode':'120000','hex':'00'}]):
            with self.subTest(rows=rows), self.assertRaises(Refused):
                snapshot_tree(rows)
