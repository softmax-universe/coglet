"""Required native CI tests must not silently pass via unittest skips."""
import io
import unittest

from required_tests import main, run_required


class RequiredTests(unittest.TestCase):
    def run_case(self, method):
        case = type('Case', (unittest.TestCase,), {'test_case': method})
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(case)
        return run_required(suite, stream=io.StringIO())

    def test_pass(self):
        self.assertEqual(self.run_case(lambda self: None), 0)

    def test_skip_fails(self):
        self.assertEqual(self.run_case(lambda self: self.skipTest('missing native prerequisite')), 1)

    def test_assertion_fails(self):
        self.assertEqual(self.run_case(lambda self: self.fail('native check failed')), 1)

    def test_error_fails(self):
        def error(self):
            raise RuntimeError('native tool missing')
        self.assertEqual(self.run_case(error), 1)

    def test_empty_suite_fails(self):
        self.assertEqual(run_required(unittest.TestSuite(), stream=io.StringIO()), 1)

    def test_no_names_fails(self):
        self.assertEqual(main([]), 1)

    def test_unknown_name_fails(self):
        suite = unittest.defaultTestLoader.loadTestsFromName('nonexistent_required_native_test')
        self.assertEqual(run_required(suite, stream=io.StringIO()), 1)
