"""Portable control-flow tests; fake keychain/codesign are NOT TCC proof."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/sign-dev.sh'

class DevSigningTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=os.environ.get('TMPDIR'))
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.app = self.root / 'Softmax.app'
        (self.app / 'Contents/Frameworks/SoftmaxCore.framework').mkdir(parents=True)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        for name, text in {
            'security': '#!/bin/bash\nprintf \'%s\\n\' "$FAKE_IDENTITIES"\n',
            'codesign': '#!/bin/bash\nprintf \'%s\\n\' "$*" >> "$SIGN_LOG"\n',
        }.items():
            path = self.bin / name
            path.write_text(text)
            path.chmod(0o755)
        self.identity = 'A' * 40
        self.env = dict(os.environ, PATH=str(self.bin)+':'+os.environ['PATH'],
                        FAKE_IDENTITIES=f'  1) {self.identity} "Apple Development: Test"',
                        SIGN_LOG=str(self.root/'sign.log'),
                        SOFTMAX_DEV_IDENTITY_FILE=str(self.root/'identity'))
        self.env.pop('SOFTMAX_DEV_SIGNING_IDENTITY', None)

    def run_sign(self):
        return subprocess.run(['bash', str(SCRIPT), str(self.app)], env=self.env, text=True, capture_output=True)

    def test_two_rebuilds_reuse_pinned_identity_and_sign_framework_first(self):
        for _ in range(2):
            result = self.run_sign()
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root/'identity').read_text().strip(), self.identity)
        lines = (self.root/'sign.log').read_text().splitlines()
        signing = [line for line in lines if '--force' in line]
        self.assertEqual(len(signing), 4)
        self.assertTrue(all('--sign '+self.identity in line for line in signing))
        self.assertIn('SoftmaxCore.framework', signing[0])
        self.assertTrue(signing[1].endswith('Softmax.app'))

    def test_bundled_hub_helper_is_signed_before_outer_app(self):
        helper = self.app / 'Contents/Helpers/softmax-hub-link'
        helper.parent.mkdir()
        helper.write_bytes(b'fixture: not a Mach-O signing proof')
        result = self.run_sign()
        self.assertEqual(result.returncode, 0, result.stderr)
        signing = [line for line in (self.root/'sign.log').read_text().splitlines() if '--force' in line]
        helper_lines = [i for i, line in enumerate(signing) if line.endswith('/Helpers/softmax-hub-link')]
        self.assertEqual(len(helper_lines), 1, signing)
        self.assertLess(helper_lines[0], len(signing) - 1)
        self.assertTrue(signing[-1].endswith('Softmax.app'))

    def test_missing_pinned_identity_fails_instead_of_changing_tcc_identity(self):
        self.assertEqual(self.run_sign().returncode, 0)
        self.env['FAKE_IDENTITIES'] = ''
        result = self.run_sign()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('pinned', result.stderr)

    def test_ambiguous_keychain_warns_and_uses_adhoc_without_pin(self):
        self.env['FAKE_IDENTITIES'] += '\n  2) '+ 'B'*40 + ' "Other"'
        result = self.run_sign()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('ad-hoc', result.stderr)
        self.assertFalse((self.root/'identity').exists())
        self.assertIn('--sign - ', (self.root/'sign.log').read_text())

if __name__ == '__main__':
    unittest.main()
