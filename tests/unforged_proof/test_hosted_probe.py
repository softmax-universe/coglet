"""Fail-closed capability-probe CLI controls, not hosted qualification."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class HostedProbeControls(unittest.TestCase):
    def invoke(self, directory, *extra):
        command = [sys.executable, str(Path(__file__).with_name('hosted_probe.py'))]
        for name in ('unforge', 'bridge', 'driver', 'recording'):
            command.extend(['--'+name, '/not-used'])
        command.extend(['--output', str(Path(directory)/'output'),
                        '--expected-endpoint', 'a'*64, '--expected-image', 'native@sha256:'+'b'*64,
                        '--expected-resolver', 'c'*64, *extra])
        env = {k:v for k,v in os.environ.items() if k not in ('CAOS_IROH_TICKET','UNFORGED_CAOS_URL')}
        return subprocess.run(command, capture_output=True, text=True, env=env, timeout=10)

    def test_unqualified_private_transport_refuses_before_output_or_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.invoke(directory)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('full transport-closure authorization not yet implemented', result.stderr)
            self.assertFalse((Path(directory)/'output').exists())

    def test_missing_hosted_config_is_unavailable_not_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.invoke(directory, '--synthetic-transport-only')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('broker ticket missing', result.stderr)
            self.assertFalse((Path(directory)/'output').exists())


if __name__ == '__main__':
    unittest.main()
