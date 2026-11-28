"""Shell syntax and explicit stub failure controls; NOT sandbox enforcement."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


def script():
    source = (Path(__file__).parent/'bridge/src/isolation.rs').read_text()
    return source.split('r#"', 1)[1].rsplit('"#;', 1)[0]


class WorkerShellControls(unittest.TestCase):
    def test_worker_shell_syntax(self):
        process = subprocess.run(['bash', '-n'], input=script(), text=True, capture_output=True)
        self.assertEqual(process.returncode, 0, process.stderr)

    def test_failed_strict_command_retained_without_fallback(self):
        with tempfile.TemporaryDirectory(prefix='stage2-shell-control-') as temp:
            root = Path(temp)
            for path in ('bin', 'args', 'capture', 'tmp'):
                (root/path).mkdir()
            (root/'args/salt').write_text('b'*64)
            unforge = root/'bin/unforge'
            unforge.write_text('#!/bin/bash\nprintf "%s\\n" "$@" > "$CALLS"\nprintf "stub sandbox refusal\\n" >&2\nexit 125\n')
            unforge.chmod(0o755)
            caos = root/'bin/caos'
            caos.write_text('#!/bin/bash\n[ "$1" = put ] || exit 99\ncp "$2" "$CAPTURE/$(basename "$3")"\n')
            caos.chmod(0o755)
            env = dict(os.environ, PATH=str(root/'bin')+os.pathsep+os.environ['PATH'],
                       CAOS_ARGS_DIR=str(root/'args'), TMPDIR=str(root/'tmp'),
                       UNFORGED_UNFORGE_BIN=str(unforge), CAOS_OWN_ARGTREE_HASH='a'*40,
                       CALLS=str(root/'calls'), CAPTURE=str(root/'capture'))
            process = subprocess.run(['bash'], input=script(), text=True, env=env,
                                     capture_output=True, timeout=10)
            self.assertEqual(process.returncode, 0, process.stderr)
            value = json.loads((root/'capture/out-trace').read_text())
            self.assertEqual(value['exit_code'], 125)
            self.assertEqual(value['inner_netns'], 'unavailable')
            self.assertEqual(value['caos_server_url_env'], 'unavailable')
            self.assertEqual((root/'capture/out').read_text(), 'stub sandbox refusal\n')
            args = (root/'calls').read_text().splitlines()
            self.assertEqual(args[:5], ['caos-sandbox-exec', '--mode', 'namespace', '--net', 'proxy-only'])
            self.assertNotIn('--proxy-sock', args)
            self.assertNotIn('--keep-fd3', args)
