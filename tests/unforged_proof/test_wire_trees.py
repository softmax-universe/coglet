"""Raw-object negative controls, not authenticated hosted execution evidence."""
import unittest
from wire_trees import object_id, verify_output, file_tree
from standard_step import Refused, snapshot_tree

class WireTrees(unittest.TestCase):
    def row(self,kind,raw):
        return dict(phase='abi-object',kind=kind,oid=object_id(kind,raw),hex=raw.hex())
    def test_empty_directory_is_not_projected_away(self):
        empty = self.row('tree',b'')
        root = self.row('tree',b'40000 extra\0'+bytes.fromhex(empty['oid']))
        self.assertNotEqual(verify_output([empty,root],root['oid']),snapshot_tree({}))
    def test_raw_blob_and_exact_root_bind_complete_tree(self):
        blob = self.row('blob',b'hello')
        root = self.row('tree',b'100644 a\0'+bytes.fromhex(blob['oid']))
        self.assertEqual(file_tree({'a':b'hello'}),root['oid'])
        self.assertEqual(verify_output([blob,root],root['oid']),snapshot_tree({'a':b'hello'}))
        with self.assertRaises(Refused): verify_output([root],root['oid'])
        with self.assertRaises(Refused): verify_output([blob,root],'a'*40)
        with self.assertRaises(Refused): verify_output([dict(blob,hex='00'),root],root['oid'])
    def test_duplicate_names_and_executable_mode_refuse(self):
        blob = self.row('blob',b'x')
        for raw in (b'100755 a\0'+bytes.fromhex(blob['oid']),
                    (b'100644 a\0'+bytes.fromhex(blob['oid']))*2):
            root = self.row('tree',raw)
            with self.assertRaises(Refused): verify_output([blob,root],root['oid'])
