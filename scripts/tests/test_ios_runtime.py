"""Source admission regressions; native archive/link execution is a separate CI gate."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class IOSRuntimeAdmissionTests(unittest.TestCase):
    def test_arm64_simulator_has_explicit_target_and_simulator_sdk(self):
        source = (ROOT / 'crates/coglet-engine-wamr/build.rs').read_text()
        self.assertIn('"aarch64-apple-ios-sim"', source)
        self.assertIn('"iphonesimulator"', source)
    def test_builder_plan_has_separate_device_and_simulator_slices(self):
        builder = ROOT / 'scripts/build-ios-runtime-xcframework.sh'
        self.assertTrue(builder.is_file(), 'iOS framework producer is missing')
        result = subprocess.run(['bash', str(builder), '--plan'], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), [
            'aarch64-apple-ios iphoneos arm64',
            'aarch64-apple-ios-sim iphonesimulator arm64',
        ])

    def test_distribution_refuses_fault_injection_request_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'must-not-exist'
            env = dict(os.environ, COGLET_RUNTIME_TEST_HOOKS='1', COGLET_IOS_XCFRAMEWORK_OUTPUT=str(output))
            result = subprocess.run(['bash', str(ROOT / 'scripts/build-ios-runtime-xcframework.sh')],
                                    env=env, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('fault-injection hooks', result.stderr)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
