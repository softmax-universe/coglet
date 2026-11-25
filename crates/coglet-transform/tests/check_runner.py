# Testing the component inventory/report guard; strings are synthetic test input,
# never execution receipts or qualification evidence.
import importlib.util
from pathlib import Path
import unittest
import contextlib
import io
import json
import tempfile
import hashlib

spec = importlib.util.spec_from_file_location("runner", Path(__file__).parents[1] / "check.py")
assert spec is not None and spec.loader is not None
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ReportGuard(unittest.TestCase):
    def test_positive(self):
        runner.validate_report("test a ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out", {"a"})

    def test_missing_renamed_ignored_duplicated_zero_and_failed_are_refused(self):
        good = "test a ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        bad = ["", good.replace("test a", "test b"), good.replace("... ok", "... ignored"), "test a ... ok\n" + good, good.replace("1 passed", "0 passed"), good.replace("0 filtered", "1 filtered"), good.replace("... ok", "... FAILED")]
        for text in bad:
            with self.subTest(text=text), self.assertRaises(AssertionError):
                runner.validate_report(text, {"a"})


class PairGuard(unittest.TestCase):
    # Deliberately synthetic byte fixtures test only the pairing validator.
    # They never leave this unittest's private directory or authorize CI trust.
    def fixtures(self, root):
        for arch in ("x86_64", "aarch64"):
            path = root / arch
            path.mkdir()
            for name in runner.REQUIRED_FILES:
                (path / name).write_bytes(b"synthetic unit input, not execution evidence")
            (path / "tool-build.jsonl").write_text(json.dumps({"reason": "compiler-artifact", "executable": "/fixture/coglet-transform", "target": {"name": "coglet-transform", "kind": ["bin"]}, "profile": {"test": False}}) + "\n")
            (path / "fuzz.results").write_bytes(b"\0" * (960 * 80))
            (path / "rustc.txt").write_text("synthetic compiler\nhost: " + arch)
            for name, expected in runner.EXPECTED.items():
                (path / f"{name}.log").write_text("\n".join(f"test {test} ... ok" for test in sorted(expected)) + f"\ntest result: ok. {len(expected)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")
            manifest = {"production_tool": "coglet-transform", "case_count": 960, "arch": arch, "mode": "native", "actual_checkout": "unit-head", "github_run": "unit-run", "github_attempt": "unit-attempt", "tests": {key: sorted(value) for key, value in runner.EXPECTED.items()}, "files": {name: hashlib.sha256((path / name).read_bytes()).hexdigest() for name in runner.REQUIRED_FILES}}
            (path / "manifest.json").write_text(json.dumps(manifest))

    def test_pair_guards(self):
        for defect in (None, "missing", "arch", "mode", "head", "run", "attempt", "hash", "tests", "corpus"):
            with self.subTest(defect=defect), tempfile.TemporaryDirectory(prefix="transform-pair-unit-") as temp:
                root = Path(temp)
                self.fixtures(root)
                path = root / "aarch64"
                manifest = json.loads((path / "manifest.json").read_text())
                if defect == "missing":
                    (path / "fuzz.corpus").unlink()
                elif defect in ("arch", "mode"):
                    manifest[defect] = "unexecuted"
                elif defect in ("head", "run", "attempt"):
                    manifest[{"head": "actual_checkout", "run": "github_run", "attempt": "github_attempt"}[defect]] = "foreign"
                elif defect == "hash":
                    manifest["files"]["qualification"] = "wrong"
                elif defect == "tests":
                    manifest["tests"] = {}
                elif defect == "corpus":
                    (path / "fuzz.corpus").write_bytes(b"different synthetic corpus")
                    manifest["files"]["fuzz.corpus"] = runner.digest(path / "fuzz.corpus")
                (path / "manifest.json").write_text(json.dumps(manifest))
                with contextlib.redirect_stdout(io.StringIO()):
                    if defect is None:
                        runner.verify_pair(root, "unit-head", "unit-run", "unit-attempt")
                    else:
                        with self.assertRaises((AssertionError, FileNotFoundError)):
                            runner.verify_pair(root, "unit-head", "unit-run", "unit-attempt")


if __name__ == "__main__":
    unittest.main()
