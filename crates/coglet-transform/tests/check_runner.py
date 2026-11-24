# Testing the component inventory/report guard; strings are synthetic test input,
# never execution receipts or qualification evidence.
import importlib.util
from pathlib import Path
import unittest

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


if __name__ == "__main__":
    unittest.main()
