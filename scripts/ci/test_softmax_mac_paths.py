import pathlib
import subprocess
import sys
import tempfile
import unittest

from softmax_mac_paths import FILES, needs_mac

ROOT = pathlib.Path(__file__).resolve().parents[2]


class MacPathsTests(unittest.TestCase):
    def test_rust_implementation_only_skips_native_build(self):
        self.assertFalse(needs_mac([
            'crates/coglet-runtime/src/database.rs',
            'crates/coglet-runtime/src/native_engine.rs',
            'crates/coglet-proto/src/lib.rs',
            'crates/coglet-mock/src/lib.rs',
            'crates/coglet-engine-wamr/src/lib.rs',
            'sdk/surface-rust/src/lib.rs',
        ]))

    def test_app_swift_and_boundary_inputs_build(self):
        for path in sorted(FILES) + [
            'apps/softmax-mac/Sources/App.swift',
            'surfaces/face-mac/Sources/Face.swift',
            'sdk/surface-swift/Sources/Surface.swift',
            'crates/coglet-runtime/include/coglet_runtime.h',
            'coglets/speech-echo/src/lib.rs',
        ]:
            with self.subTest(path=path):
                self.assertTrue(needs_mac([path]))

    def test_empty_docs_and_prefix_lookalikes_skip(self):
        self.assertFalse(needs_mac([]))
        self.assertFalse(needs_mac(['docs/design.md', 'apps/softmax-mac-fake/x',
                                    'crates/coglet-runtime/src/ffi.rs.bak']))

    def test_mixed_changes_and_unusual_names(self):
        self.assertTrue(needs_mac(['crates/coglet-proto/src/lib.rs',
                                  'apps/softmax-mac/with space\nand newline.swift']))

    def test_real_git_rename_out_of_app_and_deleted_boundary_still_build(self):
        with tempfile.TemporaryDirectory() as tmp:
            def git(*args):
                return subprocess.check_output(['git', '-C', tmp, *args])
            git('init', '-q')
            git('config', 'user.email', 'ci@example.invalid')
            git('config', 'user.name', 'CI fixture')
            path = pathlib.Path(tmp) / 'apps/softmax-mac/old.swift'
            path.parent.mkdir(parents=True)
            path.write_text('// fixture\n')
            git('add', '.'); git('commit', '-qm', 'base')
            path.rename(pathlib.Path(tmp) / 'moved.swift')
            git('add', '-A')
            names = git('diff', '--cached', '--name-only', '--no-renames', '-z')
            result = subprocess.check_output(
                [sys.executable, str(ROOT / 'scripts/ci/softmax_mac_paths.py')], input=names)
            self.assertEqual(result, b'true\n')

    def test_gate_shell_fails_closed_and_accepts_only_legitimate_skip(self):
        import os
        workflow = (ROOT / '.github/workflows/softmax-mac.yml').read_text()
        gate = workflow.split('  softmax-mac-gate:', 1)[1].split('  macos:', 1)[0]
        script = gate.split('        run: |\n', 1)[1]
        script = '\n'.join(line[10:] for line in script.splitlines())
        for paths, selected, build, expected in [
            ('success', 'true', 'success', True),
            ('success', 'false', 'skipped', True),
            ('failure', '', 'skipped', False),
            ('cancelled', 'false', 'skipped', False),
            ('success', 'true', 'failure', False),
            ('success', 'true', 'skipped', False),
            ('success', '', 'skipped', False),
            ('success', 'false', 'success', False),
        ]:
            with self.subTest(paths=paths, selected=selected, build=build):
                env = dict(os.environ, PATHS_RESULT=paths, BUILD_SELECTED=selected, BUILD_RESULT=build)
                result = subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', script], env=env)
                self.assertEqual(result.returncode == 0, expected)

    def test_workflow_runs_on_main_and_merge_group_without_path_filter(self):
        workflow = (ROOT / '.github/workflows/softmax-mac.yml').read_text()
        triggers = workflow.split('permissions:', 1)[0]
        self.assertIn('  merge_group:', triggers)
        self.assertIn('branches: [main,', triggers)
        self.assertNotIn('paths:', triggers)
        self.assertIn('build=true', workflow)
        self.assertIn('if [[ "$EVENT_NAME" == pull_request ]]', workflow)
        self.assertIn('needs: changes', workflow)
        self.assertIn("if: needs.changes.outputs.macos == 'true'", workflow)
        self.assertIn('set -euo pipefail', workflow)


if __name__ == '__main__':
    unittest.main()
