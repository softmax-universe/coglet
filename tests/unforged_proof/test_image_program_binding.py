import unittest
from pathlib import Path
from verify_image_probe import verify_programs,Refused

class ImageProgramBindingTests(unittest.TestCase):
    def fixture(self):
        return 'pub const WORKER:&str = r#"worker bytes\n"#;\npub const LAUNCHER:&str = r#"launcher bytes\n"#;'
    def leaves(self):return {'worker1':('100755',b'worker bytes\n'),'launcher':('100644',b'launcher bytes\n')}
    def test_reviewed_program_bytes_and_modes(self):verify_programs(self.leaves(),self.fixture())
    def test_coherent_replacement_cannot_hide_behind_same_args(self):
        for name in ('worker1','launcher'):
            leaves=self.leaves();leaves[name]=(leaves[name][0],b'exit 125\n')
            with self.subTest(name=name),self.assertRaises(Refused):verify_programs(leaves,self.fixture())
    def test_modes_missing_and_ambiguous_source_refuse(self):
        leaves=self.leaves();leaves['worker1']=('100644',leaves['worker1'][1])
        for data,text in [(leaves,self.fixture()),({},self.fixture()),(self.leaves(),self.fixture()+self.fixture()),(self.leaves(),'')]:
            with self.assertRaises(Refused):verify_programs(data,text)
