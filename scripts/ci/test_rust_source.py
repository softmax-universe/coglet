#!/usr/bin/env python3
"""Executable workflow/source compatibility tests; git fixtures are not CI proof."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

import re

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("rust_source", Path(__file__).with_name("rust_source.py"))
assert spec is not None and spec.loader is not None
selector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selector)


class GitFixture(unittest.TestCase):
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


class SourceSelection(GitFixture):
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


class ProfileGeneration(GitFixture):
    def setUp(self):
        super().setUp()
        for name in selector.PROFILE_INPUTS:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture input")
        self.profile = self.commit("profile generation", "profile-marker", "introduced")

    def required(self, selected, boundary=None):
        return selector.profile_required(self.repo, selected, boundary=boundary or self.profile)

    def test_pre_feature_exact_head_stays_exact_and_is_not_profile_qualified(self):
        self.git("switch", "-c", "pre-profile-pr", self.modern)
        head = self.commit("pre-profile head", "head", "change")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "pre-profile merge", head)
        selected, scope = self.choose("pull_request", self.git("rev-parse", "HEAD"), head)
        self.assertEqual((selected, scope), (head, "exact-head"))
        self.assertFalse(self.required(selected))
        self.assertFalse(self.required(self.contract))
        self.assertTrue(self.required(self.git("rev-parse", "HEAD")))

    def test_modern_exact_and_push_merge_group_require_profile(self):
        self.git("switch", "-c", "profile-pr")
        head = self.commit("profile head", "head", "change")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "profile merge", head)
        selected, scope = self.choose("pull_request", self.git("rev-parse", "HEAD"), head)
        self.assertEqual((selected, scope), (head, "exact-head"))
        self.assertTrue(self.required(selected))
        for event in ("push", "merge_group"):
            self.assertEqual(self.choose(event, head), (head, "exact-head"))
            self.assertTrue(self.required(head))

    def test_legacy_merge_integration_retains_actual_merge_identity_and_profile(self):
        self.git("switch", "-c", "another-legacy", self.old)
        head = self.commit("new legacy change", "another-legacy", "change")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "modern legacy merge", head)
        merge = self.git("rev-parse", "HEAD")
        selected, scope = self.choose("pull_request", merge, head)
        self.assertEqual((selected, scope), (merge, "merge-integration-only"))
        self.assertTrue(self.required(selected))

    def test_every_modern_missing_input_refuses_instead_of_skipping(self):
        for name in selector.PROFILE_INPUTS:
            with self.subTest(name=name):
                self.git("reset", "--hard", self.profile)
                self.git("rm", name)
                self.git("commit", "-qm", "delete required input")
                with self.assertRaises(ValueError):
                    self.required(self.git("rev-parse", "HEAD"))

    def test_nonregular_modern_inputs_refuse(self):
        name = selector.PROFILE_INPUTS[0]
        for kind in ("directory", "symlink"):
            with self.subTest(kind=kind):
                self.git("reset", "--hard", self.profile)
                path = self.repo / name
                path.unlink()
                if kind == "directory":
                    path.mkdir()
                    (path / "child").write_text("not a bridge")
                else:
                    path.symlink_to("profile-marker")
                self.git("add", ".")
                self.git("commit", "-qm", "replace required input")
                with self.assertRaises(ValueError):
                    self.required(self.git("rev-parse", "HEAD"))
                # reset --hard restores the tracked regular input next iteration.

    def test_bad_selected_or_boundary_objects_fail_closed(self):
        for bad in ("main", "", "f" * 40, "--help"):
            with self.subTest(bad=bad):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    selector.profile_required(self.repo, self.profile, boundary=bad)
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    self.required(bad)

    def test_shallow_history_cannot_classify_feature_as_absent(self):
        shallow = self.repo / "shallow"
        self.git("clone", "--depth", "1", self.repo.as_uri(), str(shallow))
        with self.assertRaises(subprocess.CalledProcessError):
            selector.profile_required(shallow, self.profile, boundary=self.contract)


class R3Generation(GitFixture):
    """R3 closure is classified by ancestry of the actual selected SHA, never by file presence."""

    def setUp(self):
        super().setUp()
        # Old coincidental files: present before the boundary, still not R3-generation.
        for name in selector.R3_INPUTS:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("coincidental")
        self.coincidental = self.commit("coincidental old files", "coincidence", "x")
        for name in selector.PROFILE_INPUTS:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("profile fixture input")
        self.profile = self.commit("profile only generation", "profile-marker", "p")
        self.r3 = self.commit("r3 generation", "r3-marker", "r3")

    def required(self, selected, boundary=None):
        return selector.r3_required(self.repo, selected, boundary=boundary or self.r3)

    def test_default_boundary_is_pr206_closure(self):
        self.assertEqual(selector.R3_BOUNDARY, "e88fc139b3fd2b43b84136f2e7a33fb8c0f33a54")
        self.assertEqual(len(selector.R3_INPUTS), 8)
        self.assertTrue(all(n.startswith("crates/coglet-hub/") for n in selector.R3_INPUTS))

    def test_old_exact_source_false_even_with_all_files_present(self):
        for name in selector.R3_INPUTS:
            self.assertEqual(self.git("ls-tree", self.coincidental, "--", name).split()[1], "blob")
        self.assertFalse(self.required(self.coincidental))
        self.assertFalse(self.required(self.contract))

    def test_profile_only_generation_precedes_r3(self):
        self.assertTrue(selector.profile_required(self.repo, self.profile, boundary=self.profile))
        self.assertFalse(self.required(self.profile))
        self.assertTrue(self.required(self.r3))

    def test_modern_exact_push_and_merge_group_true(self):
        self.git("switch", "-c", "r3-pr")
        head = self.commit("r3 head", "head", "change")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "r3 merge", head)
        merge = self.git("rev-parse", "HEAD")
        selected, scope = self.choose("pull_request", merge, head)
        self.assertEqual((selected, scope), (head, "exact-head"))
        self.assertTrue(self.required(selected))
        for event in ("push", "merge_group"):
            self.assertEqual(self.choose(event, head), (head, "exact-head"))
            self.assertTrue(self.required(head))

    def test_legacy_merge_integration_classifies_actual_merge(self):
        self.git("switch", "-c", "r3-legacy", self.old)
        head = self.commit("legacy again", "legacy-again", "c")
        self.git("switch", "modern")
        self.git("merge", "--no-ff", "-m", "legacy onto r3", head)
        selected, scope = self.choose("pull_request", self.git("rev-parse", "HEAD"), head)
        self.assertEqual(scope, "merge-integration-only")
        self.assertTrue(self.required(selected))

    def test_every_modern_deletion_refuses(self):
        for name in selector.R3_INPUTS:
            with self.subTest(name=name):
                self.git("reset", "--hard", self.r3)
                self.git("rm", name)
                self.git("commit", "-qm", "delete required input")
                with self.assertRaises(ValueError):
                    self.required(self.git("rev-parse", "HEAD"))

    def test_every_modern_nonregular_input_refuses(self):
        for name in selector.R3_INPUTS:
            for kind in ("directory", "symlink"):
                with self.subTest(name=name, kind=kind):
                    self.git("reset", "--hard", self.r3)
                    path = self.repo / name
                    path.unlink()
                    if kind == "directory":
                        path.mkdir()
                        (path / "child").write_text("not regular")
                    else:
                        path.symlink_to("r3-marker")
                    self.git("add", "-A")
                    self.git("commit", "-qm", "replace required input")
                    with self.assertRaises(ValueError):
                        self.required(self.git("rev-parse", "HEAD"))

    def test_unknown_or_malformed_objects_fail_closed(self):
        for bad in ("main", "", "f" * 40, "--help"):
            with self.subTest(bad=bad):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    selector.r3_required(self.repo, self.r3, boundary=bad)
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    self.required(bad)

    def test_shallow_history_with_available_boundary_hits_shallow_guard(self):
        self.git("config", "uploadpack.allowAnySHA1InWant", "true")
        shallow = self.repo / "shallow"
        self.git("clone", "--depth", "1", self.repo.as_uri(), str(shallow))
        subprocess.run(["git", "-C", str(shallow), "fetch", "--depth=1", self.repo.as_uri(), self.r3], check=True, capture_output=True)
        self.assertEqual(selector.commit(shallow, self.r3), self.r3)
        self.assertEqual(selector.git(shallow, "rev-parse", "--is-shallow-repository"), "true")
        with self.assertRaisesRegex(ValueError, "complete source ancestry"):
            selector.r3_required(shallow, self.r3, boundary=self.r3)


class WorkflowBinding(unittest.TestCase):
    def test_profile_flag_binds_only_profile_step_and_failure_artifact(self):
        workflow = (ROOT / ".github/workflows/rust.yml").read_text()
        self.assertIn("profile-required: ${{ steps.select.outputs.profile-required }}", workflow)
        self.assertIn("profile-boundary: ${{ steps.select.outputs.profile-boundary }}", workflow)
        steps = workflow.split("      - ")
        flagged = [s for s in steps if "needs.source.outputs.profile-required" in s]
        self.assertEqual(len(flagged), 2)
        qualification = next(s for s in flagged if "Production runtime selected-profile" in s)
        self.assertIn("if: needs.source.outputs.profile-required == 'true'\n", qualification)
        self.assertIn("python crates/coglet-runtime/tools/test_profile_fixture_bridge.py", qualification)
        artifact = next(s for s in flagged if "runtime-surface02-" in s)
        self.assertIn("if: needs.source.outputs.profile-required == 'true' && always()", artifact)
        self.assertIn("if-no-files-found: error", artifact)


    def test_all_producers_and_aggregate_share_selection_and_native_is_required(self):
        # Deliberately strict layout pins, not a general YAML parser. Keep these
        # stdlib-only: dependency-restore discovers every test_*.py before setup.
        workflow = (ROOT / ".github/workflows/rust.yml").read_text()
        jobs = dict(re.findall(r"^  ([\w-]+):\n((?:^ {4}.*\n|^\n)*)", workflow, re.M))
        selection = jobs["source"]
        self.assertIn("          ref: ${{ github.sha }}\n          fetch-depth: 0", selection)
        self.assertIn("run: python3 scripts/ci/rust_source.py", selection)
        needs = {
            "transform-native": "source",
            "transform-publication": "source",
            "transform-runtime-arm": "source, transform-publication",
            "rust-core": "source, transform-native, transform-publication, transform-runtime-arm",
            "factory": "source",
        }
        for name, dependencies in needs.items():
            job = jobs[name]
            self.assertTrue(job.startswith(f"    needs: [{dependencies}]\n"))
            self.assertEqual(job.count("uses: actions/checkout@v4"), 1)
            self.assertIn("          ref: ${{ needs.source.outputs.sha }}", job)
        native = jobs["transform-native"]
        self.assertNotRegex(native, r"(?m)^\s+if:")
        self.assertEqual(re.findall(r"^            arch: (\w+)$", native, re.M), ["x86_64", "aarch64"])
        self.assertIn("check.py --native-leg", native)
        aggregate = jobs["rust-core"]
        self.assertIn("    if: always()", aggregate)
        self.assertIn("test '${{ needs.source.result }}' = 'success'", aggregate)
        self.assertIn("test '${{ needs.transform-native.result }}' = 'success'", aggregate)
        for name in ("transform-publication", "transform-runtime-arm"):
            self.assertNotRegex(jobs[name], r"(?m)^    if:")
            self.assertIn(f"test '${{{{ needs.{name}.result }}}}' = 'success'", aggregate)
        self.assertIn("check_r3.py --publication", jobs["transform-publication"])
        self.assertIn("check_r3.py --runtime", jobs["transform-publication"])
        self.assertIn("check_r3.py --runtime", jobs["transform-runtime-arm"])
        self.assertIn("check_r3.py --aggregate", aggregate)
        self.assertLess(aggregate.index("needs.source.result"), aggregate.index("uses: actions/checkout"))
        self.assertIn("--aggregate-test", aggregate)
        self.assertIn("  merge_group:", workflow)


    def test_r3_reporting_shell_fails_closed_on_missing_or_invalid_output(self):
        workflow = (ROOT / ".github/workflows/rust.yml").read_text()
        reports = workflow.split("      - name: Report R3 generation qualification\n")[1:]
        self.assertEqual(len(reports), 2)
        for report in reports:
            block = report.split("        run: |\n", 1)[1].split("      - ", 1)[0]
            script = "\n".join(line[10:] for line in block.splitlines())
            for value in ("true", "false", "", "TRUE", "unknown"):
                with self.subTest(value=value), tempfile.TemporaryDirectory(prefix="r3-report-") as temp:
                    import os
                    summary = Path(temp) / "summary"
                    result = subprocess.run(["bash", "-e", "-c", script], capture_output=True, text=True,
                                            env={**os.environ, "R3_REQUIRED": value, "R3_SHA": "a" * 40,
                                                 "R3_BOUNDARY": selector.R3_BOUNDARY,
                                                 "GITHUB_STEP_SUMMARY": str(summary)})
                    if value in ("true", "false"):
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertIn("a" * 40, summary.read_text())
                        self.assertEqual("NOT EXECUTED/QUALIFIED" in result.stdout, value == "false")
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn("Invalid or missing", result.stderr)
                        self.assertFalse(summary.exists())

    def test_r3_flag_gates_only_actual_r3_steps_and_every_job_reports(self):
        workflow = (ROOT / ".github/workflows/rust.yml").read_text()
        self.assertIn("r3-required: ${{ steps.select.outputs.r3-required }}", workflow)
        self.assertIn("r3-boundary: ${{ steps.select.outputs.r3-boundary }}", workflow)
        jobs = dict(re.findall(r"^  ([\w-]+):\n((?:^ {4}.*\n|^\n)*)", workflow, re.M))
        gate = "\n        if: needs.source.outputs.r3-required == 'true'\n"

        def steps(name):
            return jobs[name].split("\n      - ")[1:]

        def gated(name):
            return [s for s in steps(name) if gate in s + "\n"]
        self.assertEqual(workflow.count("needs.source.outputs.r3-required == 'true'"), 9)
        expected = {
            "transform-publication": ["check_r3.py --publication", "check_r3.py --runtime", "name: r3-publication-", "name: r3-runtime-x86_64-"],
            "transform-runtime-arm": ["name: r3-publication-", "check_r3.py --runtime", "name: r3-runtime-aarch64-"],
            "rust-core": ["pattern: r3-runtime-*", "check_r3.py --aggregate"],
        }
        for name, needles in expected.items():
            found = gated(name)
            self.assertEqual(len(found), len(needles), name)
            for step, needle in zip(found, needles):
                self.assertIn(needle, step)
                self.assertNotIn("&&", step.split("needs.source.outputs.r3-required == 'true'")[1].split("\n")[0])
        # Reporting is the first step of publication and ARM jobs, never gated.
        for name in ("transform-publication", "transform-runtime-arm"):
            first = steps(name)[0]
            self.assertIn("NOT EXECUTED/QUALIFIED", first)
            self.assertIn("needs.source.outputs.sha", first)
            self.assertIn("needs.source.outputs.r3-boundary", first)
            self.assertIn("needs.source.outputs.r3-required", first)
            self.assertNotIn("if:", first)
            self.assertEqual(len([s for s in steps(name) if s.startswith("uses: actions/checkout@v4")]), 1)
        # Native pair, profile, publication/ARM jobs and strict success checks are not R3-gated.
        for step in steps("rust-core"):
            if "transform-native" in step or "profile-required" in step or "needs.source.result" in step:
                self.assertNotIn("r3-required", step)
        self.assertNotIn("r3-required", jobs["rust"])
        for name in ("transform-native", "factory"):
            self.assertNotIn("r3-required", jobs[name])
        for name in ("source", "transform-native", "transform-publication", "transform-runtime-arm", "rust-core", "factory"):
            self.assertIn(f"test '${{{{ needs.{name}.result }}}}' = 'success'", jobs["rust"])


if __name__ == "__main__":
    unittest.main()
