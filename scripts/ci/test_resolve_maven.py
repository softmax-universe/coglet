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
            with self.subTest(status=status), patch.object(module, 'seed_maven', return_value=False) as seed, patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], status)) as run:
                self.assertEqual(module.resolve(prepare=False), status)
                self.assertEqual(run.call_count, 1)
                command = run.call_args.args[0]
                self.assertIn('--offline', command)
                self.assertNotIn('-U', command)
                self.assertIn('org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline', command)
                self.assertLessEqual(run.call_args.kwargs['timeout'], 165)
                self.assertEqual(seed.call_count, status)

    def test_only_explicit_preparation_can_populate_a_miss_with_one_shared_budget(self):
        module = self.load()
        results = [subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 0)]
        with patch.object(module, 'seed_maven', return_value=False), patch.object(module.subprocess, 'run', side_effect=results) as run, patch.object(module.time, 'monotonic', side_effect=[0, 1, 40, 150]):
            self.assertEqual(module.resolve(prepare=True), 0)
            self.assertEqual(run.call_count, 3)
            self.assertIn('--offline', run.call_args_list[0].args[0])
            self.assertNotIn('--offline', run.call_args_list[1].args[0])
            self.assertIn('--offline', run.call_args_list[2].args[0])
            self.assertEqual([call.kwargs['timeout'] for call in run.call_args_list], [30, 125, 15])
    def test_prepare_can_seed_then_verify_offline_without_contacting_central(self):
        module = self.load()
        self.assertTrue(hasattr(module, 'seed_maven'), 'reviewed Maven bootstrap is missing')
        with patch.object(module, 'seed_maven', return_value=True) as seed, patch.object(module.subprocess, 'run', side_effect=[subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], 0)]) as run:
            self.assertEqual(module.resolve(prepare=True), 0)
            seed.assert_called_once()
            self.assertEqual(run.call_count, 2)
            self.assertTrue(all('--offline' in call.args[0] for call in run.call_args_list))

    def test_seed_rejects_corrupt_bytes_without_replacing_the_existing_cache(self):
        import os
        import tempfile
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory) / 'repository'
            repository.mkdir()
            (repository / 'sentinel').write_text('untouched')
            def corrupt_download(command, **kwargs):
                (Path(command[-1]) / command[-3]).write_bytes(b'corrupt fixture')
                return subprocess.CompletedProcess(command, 0)
            with patch.dict(os.environ, {'GH_TOKEN': 'non-secret-test-fixture'}), patch.object(module.subprocess, 'run', side_effect=corrupt_download):
                self.assertFalse(module.seed_maven(repository, module.time.monotonic() + 10))
            self.assertEqual((repository / 'sentinel').read_text(), 'untouched')

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
        with patch.object(module, 'seed_maven', return_value=False), patch.object(module.subprocess, 'run', side_effect=subprocess.TimeoutExpired('mvn', 165)) as run:
            self.assertEqual(module.resolve(), 1)
            self.assertEqual(run.call_count, 1)

    def test_normal_miss_restores_pinned_seed_then_resolves_offline_only(self):
        # merge_group cannot read pull_request caches; a cache-key change must still pass.
        module = self.load()
        for retry, expected in [(0, 0), (1, 1)]:
            with self.subTest(retry=retry), patch.object(module, 'seed_maven', return_value=True) as seed, patch.object(module.subprocess, 'run', side_effect=[subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], retry)]) as run:
                self.assertEqual(module.resolve(prepare=False), expected)
                seed.assert_called_once()
                self.assertEqual(run.call_count, 2)
                self.assertTrue(all('--offline' in call.args[0] for call in run.call_args_list))

    def test_proof_never_restores_the_seed_or_goes_online(self):
        import os
        module = self.load()
        with patch.object(module, 'seed_maven', return_value=True) as seed, patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)) as run:
            self.assertEqual(module.resolve(prepare=False, seed=False), 1)
            seed.assert_not_called()
            self.assertEqual(run.call_count, 1)
        with patch.object(module, 'resolve', return_value=1) as resolve, patch.object(__import__('sys'), 'argv', ['resolve_maven.py', '--proof']), patch.dict(os.environ, {}, clear=False):
            self.assertEqual(module.main(), 1)
            self.assertEqual(resolve.call_args.kwargs, {'seed': False})

    def test_exhausted_prepare_budget_cannot_start_another_attempt(self):
        module = self.load()
        with patch.object(module, 'seed_maven', return_value=False), patch.object(module.time, 'monotonic', side_effect=[0, 1, 166]), patch.object(module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)) as run:
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
            # No GH_TOKEN: the pinned seed is unavailable, so this is a true empty cache.
            env = {k: v for k, v in __import__('os').environ.items() if k != 'GH_TOKEN'}
            result = subprocess.run([__import__('sys').executable, str(root / 'scripts/ci/resolve_maven.py'),
                                     '--repository', repository], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=30)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('offline', result.stdout + result.stderr)
            print(f'Real Maven empty-cache offline refusal: {time.monotonic() - started:.2f}s', flush=True)


if __name__ == '__main__':
    unittest.main()
