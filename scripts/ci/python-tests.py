#!/usr/bin/env python3
"""Discover codegen tests, skipping owned Python quarantines unless --only-quarantined.

Only this required-check entry point skips tests; ordinary unittest and direct Go
invocations still execute everything. Stability selects held tests explicitly.
"""
import argparse
from pathlib import Path
import sys
import unittest

from quarantine import load

ROOT = Path(__file__).resolve().parents[2]


def leaves(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from leaves(item)
        else:
            yield item


def select(tests, held, only=False):
    tests = list(tests)
    missing = held - {test.id() for test in tests}
    if missing:
        raise ValueError(f'unmatched Python quarantine ids: {sorted(missing)}')
    selected = unittest.TestSuite()
    for test in tests:
        if only:
            if test.id() in held:
                selected.addTest(test)
        else:
            if test.id() in held:
                # Instance-local skip: leave the original class/method untouched.
                def skip(reason='quarantined (ci/quarantine.toml)'):
                    raise unittest.SkipTest(reason)
                setattr(test, test._testMethodName, skip)
            selected.addTest(test)
    return selected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--only-quarantined', action='store_true')
    args = parser.parse_args()
    held = {entry['id'] for entry in load()['python']}
    suite = unittest.defaultTestLoader.discover(str(ROOT / 'codegen'))
    try:
        suite = select(leaves(suite), held, args.only_quarantined)
    except ValueError as error:
        parser.error(str(error))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return int(not result.wasSuccessful())


if __name__ == '__main__':
    sys.exit(main())
