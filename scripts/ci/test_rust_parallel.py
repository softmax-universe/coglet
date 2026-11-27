"""Fail-closed parallel qualification contracts; hosted timings remain CI evidence."""
import importlib.util
from pathlib import Path
import re
import subprocess

import unittest

ROOT = Path(__file__).resolve().parents[2]


def jobs():
    return dict(re.findall(r"^  ([\w-]+):\n((?:^ {4}.*\n|^\n)*)",
                           (ROOT / '.github/workflows/rust.yml').read_text().split('\njobs:\n', 1)[1], re.M))


class ParallelQualification(unittest.TestCase):
    def test_exact_source_and_independent_runner_dag(self):
        workflow = jobs()
        self.assertEqual(set(workflow), {'source', 'transform-native', 'transform-publication', 'transform-runtime-arm', 'factory', 'rust-core', 'rust'})
        self.assertIn('    needs: [source]\n', workflow['factory'])
        self.assertIn('    needs: [source, transform-native, transform-publication, transform-runtime-arm]\n', workflow['rust-core'])
        for name in ('factory', 'rust-core'):
            job = workflow[name]
            self.assertIn('    runs-on: ubuntu-24.04\n', job)
            self.assertEqual(job.count('uses: actions/checkout@v4'), 1)
            self.assertIn('ref: ${{ needs.source.outputs.sha }}', job)
            self.assertNotIn('continue-on-error', job)
        aggregate = workflow['rust']
        self.assertIn('    needs: [source, transform-native, transform-publication, transform-runtime-arm, rust-core, factory]\n', aggregate)
        self.assertIn('    if: always()\n', aggregate)
        self.assertNotIn('actions/checkout', aggregate)

    def test_required_aggregate_executes_and_rejects_every_non_success(self):
        aggregate = jobs()['rust']
        shell = aggregate.split('        run: |\n', 1)[1]
        dependencies = ('source', 'transform-native', 'transform-publication', 'transform-runtime-arm', 'rust-core', 'factory')
        def run(values):
            script = shell
            for dep in dependencies:
                token = '${{ needs.' + dep + '.result }}'
                self.assertEqual(script.count(token), 1)
                script = script.replace(token, values.get(dep, 'success'))
            self.assertNotIn('${{', script)
            return subprocess.run(['bash', '-e', '-c', script], capture_output=True).returncode
        self.assertEqual(run({}), 0)
        for dep in dependencies:
            for result in ('failure', 'cancelled', 'skipped', ''):
                with self.subTest(dependency=dep, result=result):
                    self.assertNotEqual(run({dep: result}), 0)

    def test_factory_entrypoint_once_and_core_coverage_retained(self):
        workflow = jobs()
        all_jobs = ''.join(workflow.values())
        self.assertEqual(all_jobs.count('run: bash coglets/factory/check.sh'), 1)
        self.assertIn('run: bash coglets/factory/check.sh', workflow['factory'])
        self.assertNotIn('coglets/factory/check.sh', workflow['rust-core'])
        core = workflow['rust-core']
        # Pin all pre-split execution steps, including multiline commands and
        # artifacts: the split moves only the owned Factory entrypoint.
        for command in (
            'cargo fmt --all -- --check',
            'cargo clippy --workspace --all-targets --locked -- -D warnings',
            'cargo build --workspace --locked',
            'check.py --aggregate-test',
            'scripts/ci/cargo-test --workspace --locked',
            'scripts/ci/cargo-test --locked -p coglet-runtime --features native-engine --test device_lifecycle',
            'scripts/ci/cargo-test --locked -p coglet-engine-wamr --no-default-features --features aot',
            'cargo build -p coglet-guest-rust --target wasm32-wasip1 --locked',
            'bash sdk/guest-rust/build-guests.sh',
            'cargo test --locked -p coglet-runtime --test sim_duplex_heard -- --ignored',
            'python3 sdk/guest-rust/tests/test_wasm.py -v',
            'python3 sdk/guest-rust/tests/build_reference_cases.py --check',
            'for engine in mock wamr; do',
            'python3 sdk/guest-rust/tests/check_reports.py',
            'name: reference-coglets-wasm-',
            'bash tests/m1/run.sh', 'bash sim/check-native.sh',
            'bash tools/visualizer/check-native.sh', 'name: visualizer-native-proof-',
            'python crates/coglet-runtime/tools/test_profile_fixture_bridge.py',
            'python -m unittest discover -s conformance/tests -p test_expect_close.py -v',
            'python conformance/run --registry conformance/impls-surface02-runtime.toml',
            'name: runtime-surface02-',
            'cargo run --quiet -p coglet-runtime --bin cogletd',
        ):
            with self.subTest(command=command):
                self.assertIn(command, core)

    def test_shared_union_closure_and_single_capture_preserve_bundle_modes(self):
        workflow = jobs()
        for name in ('rust-core', 'factory'):
            job = workflow[name]
            self.assertEqual(job.count('uses: ./.github/actions/dependency-restore'), 1)
            self.assertIn('job: rust\n', job)
            self.assertNotIn('job: factory', job)
            self.assertIn("mode: ${{ inputs.bundle-mode || 'normal' }}", job)
            self.assertIn("release: ${{ inputs.bundle-release || 'dependency-mirror-v1' }}", job)
            self.assertIn("enable-cache: ${{ inputs.bundle-mode != 'proof' }}", job)
        self.assertNotIn('dependency-capture', workflow['factory'])
        core = workflow['rust-core']
        self.assertEqual(core.count('uses: ./.github/actions/dependency-capture'), 1)
        fetch = "if: inputs.bundle-mode == 'prepare'\n        run: cargo fetch --locked --manifest-path coglets/factory/Cargo.toml"
        self.assertIn(fetch, core)
        self.assertLess(core.index(fetch), core.index('uses: ./.github/actions/dependency-capture'))
        self.assertIn("uses: ./.github/actions/dependency-capture\n        if: inputs.bundle-mode == 'prepare'\n        with:\n          job: rust", core)
        capture = (ROOT / '.github/actions/dependency-capture/action.yml').read_text()
        self.assertIn('name: dependency-${{ inputs.job }}', capture)
        restore = (ROOT / '.github/actions/dependency-restore/action.yml').read_text()
        self.assertIn('required=(--required)', restore)
        self.assertIn("if: inputs.mode == 'proof'", restore)
        self.assertIn('scripts/ci/deny_registries.py', restore)
        mirror = (ROOT / '.github/workflows/dependency-mirror.yml').read_text()
        self.assertIn('needs: [prepare-rust, prepare-conformance]', mirror)
        self.assertNotIn('continue-on-error', mirror)

    def test_shared_rust_bundle_restoration_is_fail_closed(self):
        spec = importlib.util.spec_from_file_location('bundle_parallel', ROOT / 'scripts/ci/dependency_bundle.py')
        assert spec is not None and spec.loader is not None
        bundle = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bundle)
        def missing(name, directory):
            raise FileNotFoundError('fixture unavailable')
        self.assertFalse(bundle.restore_one('rust-cargo-fixture', {}, missing))
        with self.assertRaisesRegex(RuntimeError, 'required dependency bundle'):
            bundle.restore_one('rust-cargo-fixture', {}, missing, required=True)


if __name__ == '__main__':
    unittest.main()
