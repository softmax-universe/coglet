"""Formatter failures must remain fatal and expose the underlying diagnostic."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class FormatterDiagnostics(unittest.TestCase):
    def test_formatter_failure_reports_stderr_without_writing_outputs(self):
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            formatter = Path(directory) / 'rustfmt'
            formatter.write_text(
                '#!/bin/sh\n'
                'printf "test formatter component unavailable\\n" >&2\n'
                'exit 1\n'
            )
            formatter.chmod(0o755)
            env = dict(os.environ, PATH=directory + os.pathsep + os.environ['PATH'])
            result = subprocess.run(
                [sys.executable, str(root / 'codegen/generate.py'), '--check'],
                cwd=root, env=env, capture_output=True, text=True,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('test formatter component unavailable', result.stderr)
        self.assertNotIn('outputs current', result.stdout)


if __name__ == '__main__':
    unittest.main()
