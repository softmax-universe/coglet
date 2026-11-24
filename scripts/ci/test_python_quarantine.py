"""Exact Python quarantine selection without suppressing neighbors or stale ids."""
import importlib.util
import io
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('python_tests', Path(__file__).with_name('python-tests.py'))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class PythonQuarantineTests(unittest.TestCase):
    def fixture(self):
        calls = []

        class Cases(unittest.TestCase):
            def test_held(self):
                calls.append('held')

            def test_neighbor(self):
                calls.append('neighbor')

        tests = list(runner.leaves(unittest.defaultTestLoader.loadTestsFromTestCase(Cases)))
        held = {test.id() for test in tests if test._testMethodName == 'test_held'}
        return Cases, tests, held, calls

    def run_suite(self, suite):
        return unittest.TextTestRunner(stream=io.StringIO()).run(suite)

    def test_required_skips_exact_id_not_neighbor_or_later_direct_run(self):
        cases, tests, held, calls = self.fixture()
        result = self.run_suite(runner.select(tests, held))
        self.assertTrue(result.wasSuccessful())
        self.assertEqual(len(result.skipped), 1)
        self.assertEqual(calls, ['neighbor'])
        result = self.run_suite(unittest.defaultTestLoader.loadTestsFromTestCase(cases))
        self.assertTrue(result.wasSuccessful())
        self.assertEqual(result.skipped, [])
        self.assertEqual(calls, ['neighbor', 'held', 'neighbor'])

    def test_only_mode_executes_held_test(self):
        _, tests, held, calls = self.fixture()
        result = self.run_suite(runner.select(tests, held, only=True))
        self.assertEqual(result.testsRun, 1)
        self.assertEqual(result.skipped, [])
        self.assertEqual(calls, ['held'])

    def test_empty_list_keeps_all_required_tests_and_no_only_tests(self):
        _, tests, _, _ = self.fixture()
        self.assertEqual(runner.select(tests, set()).countTestCases(), 2)
        self.assertEqual(runner.select(tests, set(), only=True).countTestCases(), 0)

    def test_unmatched_is_fatal_in_both_modes(self):
        for only in (True, False):
            with self.assertRaisesRegex(ValueError, 'unmatched'):
                runner.select([], {'test_no.Such.test_case'}, only)
