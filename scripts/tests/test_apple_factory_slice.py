#!/usr/bin/env python3
"""Fail-closed selection and command wiring for the bounded Apple Factory lanes."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import subprocess

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("apple_slice", ROOT / "scripts/test_apple_factory_slice.py")
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SliceTests(unittest.TestCase):
    def names(self):
        return ([f"factory_goals::case{i}" for i in range(6)]
                + [f"factory_research::case{i}" for i in range(4)]
                + ["factory_research::backlog::case"]
                + [f"base_case{i}" for i in range(8)])

    def test_complete_disjoint_manifest(self):
        groups = module.partition(self.names())
        flat = [name for group in groups.values() for name in group]
        self.assertCountEqual(flat, self.names())
        self.assertEqual(len(flat), len(set(flat)))

    def test_missing_new_or_duplicate_tests_refuse(self):
        for names in ([], self.names()[:-1], self.names() + ["new_case"],
                      self.names() + [self.names()[0]]):
            with self.subTest(names=names), self.assertRaises(ValueError):
                module.partition(names)

    def test_exact_nonzero_commands_use_native_engine(self):
        for slice_name in module.COUNTS:
            listing = "\n".join(name + ": test" for name in self.names())
            with self.subTest(slice=slice_name), patch("sys.argv", ["slice", slice_name]), \
                    patch.object(module.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, stdout=listing)
                module.main()
                command = run.call_args_list[1].args[0]
                self.assertIn("native-engine", command)
                self.assertIn("--exact", command)
                self.assertEqual(command[len(module.CARGO):-2], module.partition(self.names())[slice_name])
                self.assertTrue(run.call_args.kwargs["check"])

    def test_failed_test_execution_propagates(self):
        listing = "\n".join(name + ": test" for name in self.names())
        with patch("sys.argv", ["slice", "goals"]), patch.object(module.subprocess, "run") as run:
            run.side_effect = [subprocess.CompletedProcess([], 0, stdout=listing),
                               subprocess.CalledProcessError(101, ["cargo"])]
            with self.assertRaises(subprocess.CalledProcessError):
                module.main()


if __name__ == "__main__":
    unittest.main()
