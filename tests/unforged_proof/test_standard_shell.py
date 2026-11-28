"""STANDARD shell stub controls, explicitly NOT real isolation evidence."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

def script():
    return (Path(__file__).parent/'bridge/src/standard.rs').read_text().split('r#"',1)[1].split('"#;',1)[0]

class StandardShell(unittest.TestCase):
    def test_syntax(self):
        p = subprocess.run(['bash','-n'],input=script(),text=True,capture_output=True)
        self.assertEqual(p.returncode,0,p.stderr)
    def test_failure_publishes_sidecar_but_never_partial_tree(self):
        with tempfile.TemporaryDirectory(prefix='stage2-standard-stub-') as temp:
            root = Path(temp)
            for path in ('bin','args/state','capture','tmp'): (root/path).mkdir(parents=True)
            (root/'args/salt').write_text('b'*64)
            # Stub controls only: let outer setup reach the original sandbox
            # refusal. No loopback/isolation claim comes from this fake driver.
            (root/'args/driver').write_text('''#!/bin/bash
case "$1" in
canary-listen)
  printf '1 1 0\\n' > "$2/stats"
  printf '12345 12346 %s\\n' "$3" > "$2/ready"
  while [ ! -f "$2/stop" ]; do sleep .01; done
  ;;
canary-positive) exit 0;;
*) exit 97;;
esac
''')
            unforge = root/'bin/unforge'
            unforge.write_text('#!/bin/bash\nprintf "stub refusal\\n" >&2\nexit 125\n')
            unforge.chmod(0o755)
            caos = root/'bin/caos'
            caos.write_text('#!/bin/bash\ncp "$2" "$CAPTURE/$(basename "$3")"\n')
            caos.chmod(0o755)
            env = dict(os.environ,PATH=str(root/'bin')+os.pathsep+os.environ['PATH'],
                       CAOS_ARGS_DIR=str(root/'args'),TMPDIR=str(root/'tmp'),
                       UNFORGED_UNFORGE_BIN=str(unforge),CAOS_OWN_ARGTREE_HASH='a'*40,CAPTURE=str(root/'capture'))
            p = subprocess.run(['bash'],input=script(),text=True,env=env,capture_output=True,timeout=10)
            self.assertEqual(p.returncode,125,p.stderr)
            self.assertEqual(json.loads((root/'capture/out-trace').read_text())['exit_code'],125)
            self.assertFalse((root/'capture/out').exists())
