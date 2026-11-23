"""Run named unittest suites as CI requirements: empty or skipped is failure."""
import sys
import unittest


def run_required(suite, stream=None):
    result = unittest.TextTestRunner(verbosity=2, stream=stream).run(suite)
    return 0 if result.wasSuccessful() and result.testsRun and not result.skipped else 1


def main(names):
    if not names:
        print('At least one required test name must be supplied', file=sys.stderr)
        return 1
    return run_required(unittest.defaultTestLoader.loadTestsFromNames(names))


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
