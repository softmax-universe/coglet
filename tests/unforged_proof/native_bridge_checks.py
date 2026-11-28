#!/usr/bin/env python3
"""Explicit built-bridge CLI checks; no network or hosted qualification."""
import argparse
import os
from pathlib import Path
import subprocess
import unittest

BRIDGE = None


class BridgeRefusals(unittest.TestCase):
    def invoke(self, *args):
        env = {k:v for k,v in os.environ.items()
               if k not in ('CAOS_IROH_TICKET', 'UNFORGED_CAOS_URL')}
        return subprocess.run([str(BRIDGE), *args], env=env,
                              capture_output=True, text=True, timeout=10)

    def test_unknown_command_refuses_before_credentials(self):
        p = self.invoke('unknown')
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('unknown bridge command', p.stderr)

    def test_sequence_requires_explicit_synthetic_ack(self):
        p = self.invoke('synthetic-cache-controls')
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('synthetic-only acknowledgement', p.stderr)

    def test_diagnostics_requires_explicit_synthetic_ack(self):
        p = self.invoke('synthetic-diagnostics')
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('synthetic-only acknowledgement', p.stderr)

    def test_diagnostics_rejects_bad_pins_before_credentials(self):
        p = self.invoke('synthetic-diagnostics', '--synthetic-transport-only',
                        'a'*64, 'native:latest', 'b'*64)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('endpoint/image/salt identities', p.stderr)

    def test_sequence_rejects_bad_identifiers_before_credentials(self):
        p = self.invoke('synthetic-cache-controls', '--synthetic-transport-only',
                        '/not-opened', 'not-an-oid', 'b'*64, 'c'*64, 'd'*64)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('full SHA-256 identities', p.stderr)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--bridge', type=Path, required=True)
    args, rest = parser.parse_known_args()
    BRIDGE = args.bridge.resolve()
    unittest.main(argv=[__file__, *rest])
