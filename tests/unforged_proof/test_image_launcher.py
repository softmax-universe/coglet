import unittest
from pathlib import Path

class ImageLauncherTests(unittest.TestCase):
    def test_cas_copy_writable_before_guest_execution(self):
        text=(Path(__file__).parent/'bridge/src/image_controls.rs').read_text()
        copy=text.index('cp -R /cas/args/state/.')
        writable=text.index('chmod -R u+w "$root/work" "$root/resolver"')
        execute=text.index('(cd "$root/work"; "$root/resolver/drivers/proof")')
        self.assertLess(copy,writable)
        self.assertLess(writable,execute)

    def test_generic_worker_does_not_require_native_only_request_environment(self):
        text=(Path(__file__).parent/'bridge/src/image_controls.rs').read_text()
        self.assertNotIn('CAOS_OWN_ARGTREE_HASH',text)
