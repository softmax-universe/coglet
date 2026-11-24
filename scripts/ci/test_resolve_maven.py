"""Maven normal-path policy; hosted proof also exercises real Maven offline."""
import importlib.util
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch


class MavenResolutionTests(unittest.TestCase):
    def load(self):
        path = Path(__file__).with_name('resolve_maven.py')
        self.assertTrue(path.exists(), 'bounded offline Maven resolver is missing')
        spec = importlib.util.spec_from_file_location('resolve_maven', path)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_normal_warm_or_empty_cache_never_uses_online_resolution(self):
        module = self.load()
        for status in [0, 1]:
            with self.subTest(status=status), patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], status)) as run:
                self.assertEqual(module.resolve(prepare=False), status)
                self.assertEqual(run.call_count, 1)
                command = run.call_args.args[0]
                self.assertIn('--offline', command)
                self.assertNotIn('-U', command)
                self.assertIn('org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline', command)
                self.assertLessEqual(run.call_args.kwargs['timeout'], 165)

    def test_only_explicit_preparation_can_populate_a_miss_with_one_shared_budget(self):
        module = self.load()
        results = [subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 0)]
        with patch.object(module.subprocess, 'run', side_effect=results) as run, patch.object(module.time, 'monotonic', side_effect=[0, 1, 40, 150]):
            self.assertEqual(module.resolve(prepare=True), 0)
            self.assertEqual(run.call_count, 3)
            self.assertIn('--offline', run.call_args_list[0].args[0])
            self.assertNotIn('--offline', run.call_args_list[1].args[0])
            self.assertIn('--offline', run.call_args_list[2].args[0])
            self.assertEqual([call.kwargs['timeout'] for call in run.call_args_list], [30, 125, 15])
    def test_prepare_does_not_force_later_test_commands_offline_before_capture(self):
        import os
        import tempfile
        module = self.load()
        for prepare in [False, True]:
            with self.subTest(prepare=prepare), tempfile.TemporaryDirectory() as directory:
                envfile = Path(directory) / 'env'
                with patch.object(module, 'resolve', return_value=0), patch.object(__import__('sys'), 'argv', ['resolve_maven.py'] + (['--prepare'] if prepare else [])), patch.dict(os.environ, {'GITHUB_ENV': str(envfile)}):
                    self.assertEqual(module.main(), 0)
                self.assertEqual(envfile.exists(), not prepare)
                if not prepare:
                    self.assertEqual(envfile.read_text(), 'MAVEN_ARGS=--offline\n')

    def test_timeout_is_failure_without_online_fallback_in_normal_mode(self):
        module = self.load()
        with patch.object(module.subprocess, 'run', side_effect=subprocess.TimeoutExpired('mvn', 165)) as run:
            self.assertEqual(module.resolve(), 1)
            self.assertEqual(run.call_count, 1)

    def test_exhausted_prepare_budget_cannot_start_another_attempt(self):
        module = self.load()
        with patch.object(module.time, 'monotonic', side_effect=[0, 1, 166]), patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)) as run:
            self.assertEqual(module.resolve(prepare=True), 1)
            self.assertEqual(run.call_count, 1)

    def test_real_maven_empty_cache_fails_offline_within_thirty_seconds(self):
        import shutil
        import tempfile
        import time
        if not shutil.which('mvn'):
            self.skipTest('Maven not installed; hosted Linux proof requires Maven')
        module = self.load()
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as repository:
            started = time.monotonic()
            result = subprocess.run([__import__('sys').executable, str(root / 'scripts/ci/resolve_maven.py'),
                                     '--repository', repository], cwd=root,
                                    capture_output=True, text=True, timeout=30)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('offline', result.stdout + result.stderr)
            print(f'Real Maven empty-cache offline refusal: {time.monotonic() - started:.2f}s', flush=True)


if __name__ == '__main__':
    unittest.main()
