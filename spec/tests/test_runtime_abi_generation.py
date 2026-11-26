"""Current ABI generators must not overwrite the independent serving corpus."""
import os
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


class RuntimeAbiGeneration(unittest.TestCase):
    def test_runtime_abi_corpus_fresh_and_spec_unchanged(self):
        paths = sorted((ROOT / 'spec/fixtures/abi').glob('*'))
        before = {p: p.read_bytes() for p in paths if p.is_file()}
        env = dict(os.environ)
        env.pop('COGLET_ABI_RUNTIME_CORPUS', None)
        env.pop('COGLET_ABI_GENERATOR_ROOT', None)
        subprocess.run([sys.executable, str(ROOT / 'spec/tests/generate_runtime_abi.py'), '--check'],
                       env=env, check=True, capture_output=True)
        self.assertEqual(before, {p: p.read_bytes() for p in before})


if __name__ == '__main__':
    unittest.main()
