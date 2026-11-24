"""Portable helper build orchestration tests, not native binary proof."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/build-hub-helper.sh'


class HubPackagingTests(unittest.TestCase):
    def test_builds_both_architectures_and_installs_verified_universal_helper(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get('TMPDIR')) as tmp:
            root = Path(tmp)
            tools = root / 'bin'
            tools.mkdir()
            for name, body in {
                'cargo': '''#!/bin/bash
set -eu
printf 'cargo %s\\n' "$*" >> "$BUILD_LOG"
target=''
while [[ $# -gt 0 ]]; do
  if [[ "$1" == '--target' ]]; then target="$2"; shift; fi
  shift
done
mkdir -p "$CARGO_TARGET_DIR/$target/release"
printf '%s' "$target" > "$CARGO_TARGET_DIR/$target/release/softmax-hub-link"
''',
                'lipo': '''#!/bin/bash
set -eu
printf 'lipo %s\\n' "$*" >> "$BUILD_LOG"
if [[ "$1" == '-create' ]]; then
  while [[ "$1" != '-output' ]]; do shift; done
  printf 'fixture universal bytes' > "$2"
else
  # Match Apple's lipo grammar: input file precedes the variadic arch list.
  [[ $# == 4 && -f "$1" && "$2" == '-verify_arch' && "$3" == 'arm64' && "$4" == 'x86_64' ]] || exit 64
fi
''',
            }.items():
                path = tools / name
                path.write_text(body)
                path.chmod(0o755)
            destination = root / 'App With Spaces.app/Contents/Helpers/softmax-hub-link'
            env = dict(os.environ, PATH=str(tools) + ':' + os.environ['PATH'],
                       CARGO_TARGET_DIR=str(root / 'target'), BUILD_LOG=str(root / 'build.log'))
            result = subprocess.run(['bash', str(SCRIPT), str(destination)], env=env,
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(destination.is_file())
            self.assertTrue(os.access(destination, os.X_OK))
            log = (root / 'build.log').read_text().splitlines()
            cargo = [line for line in log if line.startswith('cargo ')]
            self.assertEqual(len(cargo), 2)
            for target in ('aarch64-apple-darwin', 'x86_64-apple-darwin'):
                self.assertTrue(any('--target ' + target in line for line in cargo))
            self.assertTrue(all('--locked' in line and '--release' in line for line in cargo))
            self.assertTrue(any(' -verify_arch arm64 x86_64' in line for line in log))


if __name__ == '__main__':
    unittest.main()
