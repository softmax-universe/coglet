#!/usr/bin/env python3
"""Partition completeness and required-context dependency regression tests."""
import copy
import importlib.util
import sys
from pathlib import Path
import unittest
from unittest.mock import patch
import yaml

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


native = load("apple-native-tests")
gate = load("apple-native-gate")


def validate_workflow(workflow):
    jobs = workflow["jobs"]
    assert set(jobs["apple"]["needs"]) == gate.REQUIRED
    assert jobs["apple"]["if"] == "always()"
    assert set(jobs) == gate.REQUIRED | {"apple"}
    for name in ("apple-runtime", "apple-factory", "apple-release-smoke"):
        assert jobs[name]["timeout-minutes"] == 25
    assert jobs["ios-runtime"]["timeout-minutes"] == 40
    assert jobs["armv7-qemu"]["timeout-minutes"] == 15
    assert jobs["apple-factory"]["strategy"]["matrix"]["shard"] == list(range(native.SHARDS))
    assert jobs["apple-factory"]["strategy"]["fail-fast"] is False
    for name in ("apple-factory", "apple-release-smoke"):
        assert jobs[name]["needs"] == "apple-runtime"
    assert "needs" not in jobs["apple-runtime"]
    for job in jobs.values():
        assert not job.get("continue-on-error", False)
        for step in job["steps"]:
            assert not step.get("continue-on-error", False)
    commands = "\n".join(step.get("run", "") for step in jobs["apple-runtime"]["steps"])
    for expected in ("apple-native-tests.py runtime", "--features aot",
                     "--target x86_64-apple-darwin", "apple-link-probe.sh",
                     "qualify_apple_guest", "cargo test --locked -p coglet-engine-wamr -p coglet-runtime -- --test-threads=1"):
        assert expected in commands
    factory = "\n".join(step.get("run", "") for step in jobs["apple-factory"]["steps"])
    assert "apple-native-tests.py factory --shard ${{ matrix.shard }}" in factory
    steps = jobs["apple"]["steps"]
    assert any("test_apple_native_tests.py" in step.get("run", "") for step in steps)
    assert any(step.get("env", {}).get("NEEDS_JSON") == "${{ toJSON(needs) }}"
               and "apple-native-gate.py" in step.get("run", "") for step in steps)


class Partitions(unittest.TestCase):
    def test_runtime_command_preserves_examples_and_doctests(self):
        metadata = {"packages": [{"name": "coglet-runtime", "features": {}, "targets": [
            {"name": "factory_database", "kind": ["test"], "test": True},
            {"name": "runtime", "kind": ["rlib"], "test": True},
            {"name": "compile_only_example", "kind": ["example"], "test": False}]}]}
        with patch.object(native, "run", side_effect=[native.json.dumps(metadata), None, None, None]) as run, \
                patch("sys.argv", ["apple-native-tests.py", "runtime"]):
            native.main()
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[1], ["cargo", "build", "--locked", "-p", "coglet-runtime",
                                       "--features", "native-engine", "--examples"])
        self.assertEqual(commands[2], native.BASE + ["--lib", "--", "--test-threads=1"])
        self.assertEqual(commands[3], native.BASE + ["--doc", "--", "--test-threads=1"])

    def test_factory_command_uses_all_exact_selected_names(self):
        names = sorted(["a", "b", "c", "d", native.ISOLATED])
        inventory = "\n".join(f"{name}: test" for name in names) + "\n5 tests, 0 benchmarks\n"
        with patch.object(native, "run", side_effect=[inventory, None]) as run, \
                patch("sys.argv", ["apple-native-tests.py", "factory", "--shard", "0"]):
            native.main()
        self.assertEqual(run.call_args_list[1].args[0], native.BASE +
                         ["--test", "factory_database", "--", "--exact", "--test-threads=1"] +
                         native.partition(names, 0))

    def test_isolated_command_and_execution_failure_propagate(self):
        names = sorted(["a", "b", "c", "d", native.ISOLATED])
        inventory = "\n".join(f"{name}: test" for name in names) + "\n5 tests, 0 benchmarks\n"
        failure = native.subprocess.CalledProcessError(9, ["cargo"])
        with patch.object(native, "run", side_effect=[inventory, failure]) as run, \
                patch("sys.argv", ["apple-native-tests.py", "factory", "--shard", "3"]):
            with self.assertRaises(native.subprocess.CalledProcessError):
                native.main()
        self.assertEqual(run.call_args_list[1].args[0], native.BASE +
                         ["--test", "factory_database", "--", "--exact", "--test-threads=1", native.ISOLATED])

    def test_subprocess_nonzero_is_not_swallowed(self):
        with self.assertRaises(native.subprocess.CalledProcessError):
            native.run([sys.executable, "-m", "sys"])

    def test_every_discovered_name_runs_exactly_once_including_future_tests(self):
        for count in (5, 14, 19, 31):
            fixture = [native.ISOLATED] + [f"module::test_{i:02}" for i in range(count - 1)]
            names = native.factory_names("\n".join(f"{name}: test" for name in fixture)
                                         + f"\n\n{count} tests, 0 benchmarks\n")
            selected = [name for i in range(native.SHARDS) for name in native.partition(names, i)]
            self.assertEqual(sorted(selected), names)
            self.assertEqual(len(set(selected)), count)
            self.assertEqual(native.partition(names, 3), [native.ISOLATED])
            for shard in range(native.ROUND_ROBIN_SHARDS):
                self.assertEqual(native.partition(names, shard),
                                 [name for name in names[shard::3] if name != native.ISOLATED])

    def test_missing_outlier_or_empty_partition_refused(self):
        for shard in range(native.SHARDS):
            with self.assertRaises(ValueError):
                native.partition(["a", "b", "c", "d"], shard)
        with self.assertRaises(ValueError):
            native.partition([native.ISOLATED, "x", "y"], 0)
        with self.assertRaises(ValueError):
            native.partition([native.ISOLATED, native.ISOLATED, "x", "y"], 3)

    def test_malformed_inventory_refused(self):
        for output in ("", "one: test\n1 test, 0 benchmarks", "a: test\nb: test\nc: test\n4 tests, 0 benchmarks",
                       "a: test\na: test\nb: test\n3 tests, 0 benchmarks", "unknown", "x: benchmark"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                native.factory_names(output)

    def test_invalid_shard_refused(self):
        for shard in (-1, native.SHARDS):
            with self.assertRaises(ValueError):
                native.partition(["a", "b", "c"], shard)

    def test_target_partition_includes_future_tests_and_preserves_cargo(self):
        targets = [{"name": name, "kind": [kind], "test": True} for name, kind in
                   (("coglet_runtime", "rlib"), ("cogletd", "bin"), ("factory_database", "test"),
                    ("new_future_test", "test"), ("test_example", "example"))]
        metadata = {"packages": [{"name": "coglet-runtime", "features": {}, "targets": targets}]}
        self.assertEqual(native.runtime_targets(metadata), ["--lib", "--bin", "cogletd", "--test", "new_future_test", "--example", "test_example"])
        targets.pop(2)
        with self.assertRaises(ValueError):
            native.runtime_targets(metadata)

    def test_required_features_match_cargo_default_native_selection(self):
        targets = [{"name": "factory_database", "kind": ["test"], "test": True},
                   {"name": "enabled", "kind": ["test"], "test": True, "required-features": ["transitive"]},
                   {"name": "opt_in", "kind": ["test"], "test": True, "required-features": ["m1-tests"]}]
        metadata = {"packages": [{"name": "coglet-runtime", "targets": targets,
                                  "features": {"default": ["daemon"], "daemon": ["transitive"],
                                               "transitive": [], "m1-tests": []}}]}
        self.assertEqual(native.runtime_targets(metadata), ["--test", "enabled"])

    def test_unknown_target_kind_refused(self):
        with self.assertRaises(ValueError):
            native.runtime_targets({"packages": [{"name": "coglet-runtime", "features": {}, "targets": [
                {"name": "new_kind", "kind": ["unknown"], "test": True}]}]})

    def test_gate_requires_each_job_success(self):
        good = {name: {"result": "success"} for name in gate.REQUIRED}
        self.assertTrue(gate.accepted(good))
        for name in good:
            missing = copy.deepcopy(good)
            del missing[name]
            self.assertFalse(gate.accepted(missing))
            for status in ("failure", "skipped", "cancelled", "", None):
                bad = copy.deepcopy(good)
                bad[name]["result"] = status
                self.assertFalse(gate.accepted(bad))
        self.assertFalse(gate.accepted({}))
        self.assertFalse(gate.accepted(None))

    def test_workflow_contract_and_negative_controls(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/wamr-platforms.yml").read_text())
        validate_workflow(workflow)
        mutations = [lambda w: w["jobs"]["apple"]["needs"].remove("apple-factory"),
                     lambda w: w["jobs"]["apple"].update({"if": "success()"}),
                     lambda w: w["jobs"]["apple-factory"]["strategy"]["matrix"].update({"shard": [0, 1]}),
                     lambda w: w["jobs"]["apple-factory"].update({"continue-on-error": True}),
                     lambda w: w["jobs"]["apple-runtime"].update({"timeout-minutes": 30}),
                     lambda w: w["jobs"]["apple-release-smoke"].update({"needs": "apple"})]
        for mutation in mutations:
            bad = copy.deepcopy(workflow)
            mutation(bad)
            with self.assertRaises(AssertionError):
                validate_workflow(bad)


if __name__ == "__main__":
    unittest.main()
