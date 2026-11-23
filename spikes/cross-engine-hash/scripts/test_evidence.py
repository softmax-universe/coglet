#!/usr/bin/env python3
"""Harness regressions using synthetic logs/tools; real engines run separately in CI."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent


def rows(value=0):
    return "".join(f"{i} {value:016x} {value:016x}\n" for i in range(1000))


class CompareTests(unittest.TestCase):
    def compare(self, a, b, expect="equal"):
        with tempfile.TemporaryDirectory() as tmp:
            paths = [Path(tmp) / n for n in ("a", "b")]
            for path, text in zip(paths, (a, b)):
                path.write_text(text)
            return subprocess.run(
                [sys.executable, str(HERE / "compare.py"), "--expect", expect,
                 f"a={paths[0]}", f"b={paths[1]}"], capture_output=True, text=True)

    def test_valid_controls(self):
        for a, b, expect, code in [(rows(), rows(), "equal", 0),
                                   (rows(), rows(1), "differ", 0),
                                   (rows(), rows(), "differ", 1),
                                   (rows(), rows(1), "equal", 1)]:
            with self.subTest(expect=expect, code=code):
                self.assertEqual(self.compare(a, b, expect).returncode, code)

    def test_invalid_evidence_never_proves_equality_or_divergence(self):
        valid = rows()
        cases = {
            "empty": "", "truncated": "".join(valid.splitlines(True)[:10]),
            "duplicate": valid + valid.splitlines(True)[0],
            "gap": valid.replace("500 ", "501 ", 1),
            "extra": valid + "1000 0000000000000000 0000000000000000\n",
            "malformed": valid + "engine failed\n", "blank": valid + "\n",
            "bad_hash": valid.replace("0000000000000000", "zz", 1),
            "guest_host": valid.replace("0000000000000000", "0000000000000001", 1),
            "bad_blob": valid.replace("\n", " abcd\n", 1),
        }
        for name, invalid in cases.items():
            for expect in ("equal", "differ"):
                for other in (invalid, valid):
                    with self.subTest(name=name, expect=expect, same=other == invalid):
                        result = self.compare(invalid, other, expect)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn("INVALID EVIDENCE", result.stderr)

    def test_valid_blob(self):
        text = rows().replace("\n", " " + "00" * 416 + "\n")
        self.assertEqual(self.compare(text, text).returncode, 0)


# A deliberately synthetic tool double, never used as cross-engine evidence.
FAKE_TOOL = r'''#!/usr/bin/env python3
import os
from pathlib import Path
import sys
args = sys.argv[1:]
kind = Path(sys.argv[0]).name
fault = os.environ.get("FAULT", "")
name = " ".join(args)
if args[0] == "lint":
    rule = next((r for n, r in [("wasi-", "R4"), ("threads", "R1"),
                                ("relaxed-simd", "R2")] if n in name), None)
    if rule:
        print(f"VIOLATION {rule} synthetic fixture")
    sys.exit(1 if rule else 0)
if args[0] == "nancanon":
    if fault == "transform": sys.exit(7)
    Path(args[2]).write_text("synthetic")
    sys.exit(0)
if kind == "wc":
    if fault == "compile": sys.exit(7)
    Path(args[1]).write_text("synthetic")
    sys.exit(0)
if kind == "x" and "relaxed-simd" in name:
    if fault == "reject-success": sys.exit(0)
    if fault == "reject-signal": sys.exit(139)
    if fault == "reject-output": print("0 0000000000000000 0000000000000000")
    print("wrong failure" if fault == "reject-wrong" else
          "load failed: WASM module load failed: invalid opcode 0xfd 105.", file=sys.stderr)
    sys.exit(1)
value = 0
if "conforming.wasm" in name or "conforming.aot" in name:
    value = int("--nan-canon on" in name or name.endswith(".aot"))
if "wasi-" in name and kind == "w":
    counter = Path(os.environ["OUT"]) / "counter"
    value = int(counter.read_text()) + 1 if counter.exists() else 1
    counter.write_text(str(value))
count = 10 if fault == "truncated" else 1000
for i in range(count): print(f"{i} {value:016x} {value:016x}")
# Failure even AFTER a full valid log must fail the matrix.
if fault == "engine" or (fault == "aot-engine" and name.endswith(".aot")):
    sys.exit(7)
'''


class MatrixTests(unittest.TestCase):
    def matrix(self, fault):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = dict(os.environ, OUT=tmp, R=str(root / "results"),
                       GITHUB_STEP_SUMMARY=str(root / "summary"), FAULT=fault)
            for key in ("W", "X", "WC"):
                tool = root / key.lower()
                tool.write_text(FAKE_TOOL)
                tool.chmod(0o755)
                env[key] = str(tool)
            return subprocess.run(["bash", str(HERE / "run-matrix.sh")],
                                  env=env, capture_output=True, text=True)

    def test_full_positive_control(self):
        result = self.matrix("")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("RESULT: all expectations met", result.stdout)

    def test_fail_closed(self):
        for fault in ("transform", "compile", "engine", "aot-engine", "truncated",
                      "reject-success", "reject-signal", "reject-wrong", "reject-output"):
            with self.subTest(fault=fault):
                result = self.matrix(fault)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertNotIn("RESULT: all expectations met", result.stdout)


if __name__ == "__main__":
    unittest.main()
