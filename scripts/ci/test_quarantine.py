"""Quarantine file validation, libtest arguments and expiry."""
import datetime
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('quarantine', HERE / 'quarantine.py')
quarantine = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(quarantine)

CARD = 't_0123456789abcdef'
GOOD = f'''
[[conformance]]
id = "surface-go-reference.surface/action-accept-outcome"
card = "{CARD}"
since = 2026-11-24
reason = "step 5 TimeoutError, passes on rerun"

[[rust]]
id = "runaway_deadline_then_fresh_success"
card = "{CARD}"
since = 2026-11-01
reason = "fuel vs wall clock race"

[[rust]]
id = "tests::poisoned_lock"
card = "{CARD}"
since = 2026-11-24
reason = "FFI recv not-ready race"
'''


class QuarantineTests(unittest.TestCase):
    def write(self, text):
        handle = tempfile.NamedTemporaryFile('w', suffix='.toml', delete=False)
        handle.write(text)
        handle.close()
        self.addCleanup(Path(handle.name).unlink)
        return Path(handle.name)

    def test_repository_file_is_valid(self):
        quarantine.load()  # the committed ci/quarantine.toml must always parse

    def test_missing_file_means_nothing_quarantined(self):
        loaded = quarantine.load(Path(tempfile.gettempdir()) / 'no-such-quarantine.toml')
        self.assertEqual(loaded, {'conformance': [], 'rust': []})
        self.assertEqual(quarantine.rust_args(loaded), [])
        self.assertEqual(quarantine.rust_args(loaded, only=True), [])

    def test_skip_and_only_arguments_are_exact(self):
        loaded = quarantine.load(self.write(GOOD))
        self.assertEqual(quarantine.rust_args(loaded),
                         ['--exact', '--skip', 'runaway_deadline_then_fresh_success', '--skip', 'tests::poisoned_lock'])
        self.assertEqual(quarantine.rust_args(loaded, only=True),
                         ['--exact', 'runaway_deadline_then_fresh_success', 'tests::poisoned_lock'])

    def test_expiry_after_fourteen_days(self):
        loaded = quarantine.load(self.write(GOOD))
        stale = quarantine.expired(loaded, datetime.date(2026, 11, 24))
        self.assertEqual([entry['id'] for _, entry in stale], ['runaway_deadline_then_fresh_success'])
        self.assertEqual(quarantine.expired(loaded, datetime.date(2026, 11, 15)), [])

    def test_rejects_entries_that_cannot_be_owned_or_matched(self):
        bad = {
            'no card': GOOD.replace(f'card = "{CARD}"', 'card = "someone"', 1),
            'string date': GOOD.replace('since = 2026-11-24', 'since = "2026-11-24"', 1),
            'empty reason': GOOD.replace('reason = "step 5 TimeoutError, passes on rerun"', 'reason = " "', 1),
            'conformance id without case': GOOD.replace('surface-go-reference.surface/action-accept-outcome',
                                                        'surface-go-reference', 1),
            'rust id with spaces': GOOD.replace('tests::poisoned_lock', 'tests poisoned', 1),
            'extra field': GOOD.replace('since = 2026-11-01', 'since = 2026-11-01\nowner = "x"', 1),
            'unknown section': GOOD + '\n[[python]]\nid = "x"\n',
            'duplicate': GOOD + f'\n[[rust]]\nid = "tests::poisoned_lock"\ncard = "{CARD}"\nsince = 2026-11-24\nreason = "dup"\n',
        }
        for name, text in bad.items():
            with self.subTest(name=name), self.assertRaises(ValueError):
                quarantine.load(self.write(text))

    def test_cli_check_fails_only_when_asked_and_expired(self):
        path = self.write(GOOD)
        script = HERE / 'quarantine.py'
        def run(*args):
            return subprocess.run([sys.executable, str(script), '--file', str(path), *args],
                                  capture_output=True, text=True)
        self.assertEqual(run('check', '--today', '2026-11-24').returncode, 0)
        failed = run('check', '--fail-expired', '--today', '2026-11-24')
        self.assertEqual(failed.returncode, 1)
        self.assertIn('EXPIRED rust runaway_deadline_then_fresh_success', failed.stderr)
        self.assertEqual(run('check', '--fail-expired', '--today', '2026-11-10').returncode, 0)
        self.assertEqual(run('rust-args', '--only').stdout.split(),
                         ['--exact', 'runaway_deadline_then_fresh_success', 'tests::poisoned_lock'])


if __name__ == '__main__':
    unittest.main()
