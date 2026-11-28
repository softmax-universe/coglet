"""LOCAL exact-byte source audit binding regressions, not hosted proof."""
import unittest
from no_effects_policy import verify_files

class SourceAuditTests(unittest.TestCase):
    def test_exact_reviewed_bytes(self):
        import hashlib
        files={'src/a.rs':b'recorded only'}
        pins={p:hashlib.sha256(b).hexdigest() for p,b in files.items()}
        self.assertEqual(verify_files(files,pins),pins)
    def test_added_deleted_or_changed_source_refuses(self):
        import hashlib
        pins={'src/a.rs':hashlib.sha256(b'recorded only').hexdigest()}
        for files in ({},{'src/a.rs':b'changed'},{'src/a.rs':b'recorded only','src/new.rs':b'new'}):
            with self.subTest(files=files),self.assertRaisesRegex(ValueError,'source audit'):
                verify_files(files,pins)
    def test_invalid_pin_cannot_waive_audit(self):
        for pins in ({},{'src/a.rs':'yes'},{'src/a.rs':True}):
            with self.subTest(pins=pins),self.assertRaises(ValueError):verify_files({'src/a.rs':b'x'},pins)

if __name__=='__main__':unittest.main()
