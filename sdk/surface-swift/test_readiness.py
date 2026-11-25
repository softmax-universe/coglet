"""Real Swift executable startup regression; build the reference binary first."""
import asyncio
import copy
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import SUITES, cases_for, registry, run_case


def implementation():
    return registry(ROOT / 'sdk/surface-swift/conformance.toml')[0]


def delayed(impl):
    impl = copy.deepcopy(impl)
    # Delay only process startup, not wire replies. exec preserves the harness's
    # process-group ownership and lets the REAL adapter emit its own readiness.
    impl['command'] = [sys.executable, '-c',
                       'import os,sys,time; time.sleep(.4); os.execv(sys.argv[1], sys.argv[1:])',
                       *impl['command']]
    return impl


class SwiftReadinessTests(unittest.TestCase):
    def setUp(self):
        self.impl = implementation()
        self.assertTrue((ROOT / self.impl['command'][0]).is_file(), 'build Swift reference first')
        self.case = next(c for c in cases_for(SUITES, 'surface') if c['name'] == 'action-accept-outcome')

    def test_slow_swift_startup_does_not_spend_step_zero(self):
        result = asyncio.run(run_case(delayed(self.impl), 'surface', self.case))
        self.assertIsNone(result[3], result)
        self.assertIsNone(result[4], result)

    def test_startup_remains_bounded(self):
        impl = delayed(self.impl)
        impl['connect_timeout_ms'] = 100
        result = asyncio.run(run_case(impl, 'surface', self.case))
        self.assertEqual(result[3], 'TimeoutError: ', result)

    def test_invalid_config_never_signals_ready(self):
        impl = copy.deepcopy(self.impl)
        impl['fixtures']['surface-base']['env']['COGLET_FIXTURE_CONFIG'] = 'missing-swift-readiness-config.json'
        result = asyncio.run(run_case(impl, 'surface', self.case))
        self.assertIn('adapter exited before readiness signal', result[3], result)
        self.assertIn('Swift reference:', result[3], result)

    def test_without_readiness_reproduces_step_zero_timeout(self):
        impl = delayed(self.impl)
        impl.pop('ready_line', None)
        result = asyncio.run(run_case(impl, 'surface', self.case))
        self.assertIn('step 0: TimeoutError', result[3], result)


if __name__ == '__main__':
    unittest.main()
