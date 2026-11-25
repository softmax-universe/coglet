#!/usr/bin/env python3
"""Executable workflow/source compatibility tests; git fixtures are not CI proof."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("rust_source", Path(__file__).with_name("rust_source.py"))
assert spec is not None and spec.loader is not None
selector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selector)


class SourceSelection(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rust-source-unit-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git("init", "-q")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "user.name", "Fixture")
        self.old = self.commit("legacy", "old", "no transform")
        self.git("switch", "-c", "modern")
        self.contract = self.commit("contract", "crates/coglet-transform/check.py", "contract")
        self.modern = self.commit("modern", "current", "current")
        self.git("switch", "-c", "legacy-pr", self.old)
        self.legacy = self.commit("legacy-pr", "legacy-pr", "change")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "synthetic merge", self.legacy)
        self.merge = self.git("rev-parse", "HEAD")

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True, text=True, capture_output=True).stdout.strip()

    def commit(self, message, name, content):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        self.git("add", ".")
        self.git("commit", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def choose(self, event, run, head=""):
        return selector.select_source(self.repo, event, run, head, contract=self.contract)

    def test_legacy_head_without_transform_uses_bound_merge_integration_only(self):
        self.assertFalse((self.repo / "missing").exists())
        self.assertEqual(self.git("ls-tree", self.legacy, "crates/coglet-transform"), "")
        self.assertEqual(self.choose("pull_request", self.merge, self.legacy), (self.merge, "merge-integration-only"))

    def test_current_transform_bearing_head_stays_exact(self):
        self.git("switch", "-c", "current-pr", self.modern)
        head = self.commit("head", "head", "head")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "current merge", head)
        merge = self.git("rev-parse", "HEAD")
        self.assertEqual(self.choose("pull_request", merge, head), (head, "exact-head"))

    def test_modern_deletion_does_not_opt_into_legacy_fallback(self):
        self.git("switch", "-c", "delete-pr", self.modern)
        self.git("rm", "crates/coglet-transform/check.py")
        self.git("commit", "-qm", "delete")
        head = self.git("rev-parse", "HEAD")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "delete merge", head)
        self.assertEqual(self.choose("pull_request", self.git("rev-parse", "HEAD"), head), (head, "exact-head"))

    def test_push_and_merge_group_use_exact_event_sha(self):
        for event in ("push", "merge_group", "workflow_dispatch"):
            self.assertEqual(self.choose(event, self.modern), (self.modern, "exact-head"))

    def test_wrong_merge_parent_is_refused(self):
        with self.assertRaises(ValueError):
            self.choose("pull_request", self.merge, self.old)

    def test_unavailable_objects_and_malformed_sha_fail_closed(self):
        for sha in ("f" * 40, "main", "", "--help"):
            with self.subTest(sha=sha), self.assertRaises((ValueError, subprocess.CalledProcessError)):
                self.choose("pull_request", self.merge, sha)

    def test_selected_source_must_contain_contract(self):
        with self.assertRaises(ValueError):
            self.choose("push", self.old)

    def test_unknown_contract_is_not_a_negative_ancestry_result(self):
        with self.assertRaises(subprocess.CalledProcessError):
            selector.select_source(self.repo, "pull_request", self.merge, self.legacy, contract="f" * 40)


class WorkflowBinding(unittest.TestCase):
    def test_all_producers_and_aggregate_share_selection_and_native_is_required(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/rust.yml").read_text())
        jobs = workflow["jobs"]
        selection = jobs["source"]
        self.assertEqual(selection["steps"][0]["with"]["ref"], "${{ github.sha }}")
        self.assertEqual(selection["steps"][0]["with"]["fetch-depth"], 0)
        self.assertIn("python3 scripts/ci/rust_source.py", selection["steps"][-1]["run"])
        for name in ("transform-native", "rust"):
            job = jobs[name]
            self.assertIn("source", job["needs"])
            checkout = next(s for s in job["steps"] if s.get("uses") == "actions/checkout@v4")
            self.assertEqual(checkout["with"]["ref"], "${{ needs.source.outputs.sha }}")
        native = jobs["transform-native"]
        self.assertNotIn("if", native)
        self.assertEqual({leg["arch"] for leg in native["strategy"]["matrix"]["include"]}, {"x86_64", "aarch64"})
        native_run = next(s["run"] for s in native["steps"] if "run" in s)
        self.assertIn("check.py --native-leg", native_run)
        aggregate = jobs["rust"]
        self.assertEqual(aggregate["if"], "always()")
        self.assertIn("needs.source.result", aggregate["steps"][0]["run"])
        self.assertIn("needs.transform-native.result", aggregate["steps"][0]["run"])
        self.assertIn("--aggregate-test", "\n".join(s.get("run", "") for s in aggregate["steps"]))
        # PyYAML's YAML1.1 parser treats the workflow key 'on' as True.
        self.assertIn("merge_group", workflow.get("on", workflow.get(True)))


if __name__ == "__main__":
    unittest.main()
