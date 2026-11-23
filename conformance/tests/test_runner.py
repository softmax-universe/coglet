"""Exercise the bootstrap dispatch, not protocol conformance."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUN = Path(__file__).resolve().parents[1] / 'run'


class RunnerTests(unittest.TestCase):
    def invoke(self, registry):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'implementations.json'
            path.write_text(json.dumps(registry))
            return subprocess.run(
                [sys.executable, str(RUN), '--registry', str(path)],
                cwd=directory, capture_output=True, text=True, check=False,
            )

    def test_empty_registry(self):
        result = self.invoke({})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('No implementations registered', result.stdout)

    def test_registered_command_runs_from_repo(self):
        result = self.invoke({'stub': [sys.executable, '-c',
            "from pathlib import Path; assert Path('Cargo.toml').is_file(); print('executed')"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('executed', result.stdout)

    def test_failure_propagates(self):
        self.assertNotEqual(self.invoke({'bad': [sys.executable, '-c', 'raise SystemExit(7)']}).returncode, 0)

    def test_invalid_registry_fails(self):
        for registry in [[], {'bad': 'echo not-an-argv'}, {'bad': []}, {'bad': [1]}]:
            with self.subTest(registry=registry):
                self.assertNotEqual(self.invoke(registry).returncode, 0)

    def test_missing_command_fails(self):
        self.assertNotEqual(self.invoke({'missing': ['./no-such-implementation']}).returncode, 0)


if __name__ == '__main__':
    unittest.main()
