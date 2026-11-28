#!/usr/bin/env python3
"""Explicit native loopback canary checks; never part of fast/fake qualification."""
import argparse
import os
import signal
import socket
import pathlib
import subprocess
import tempfile
import time
import unittest

DRIVER = ''

class CanaryChecks(unittest.TestCase):
    def test_outer_positive_and_unisolated_probe_refuses(self):
        with tempfile.TemporaryDirectory() as td:
            root = pathlib.Path(td)
            server = subprocess.Popen([DRIVER, 'canary-listen', str(root), 'a'*64], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                for _ in range(100):
                    if (root/'ready').exists() or server.poll() is not None:
                        break
                    time.sleep(.02)
                self.assertTrue((root/'ready').exists(), 'listener readiness unavailable')
                config = root/'ready'
                good = subprocess.run([DRIVER, 'canary-positive', str(config)], capture_output=True, text=True, timeout=5)
                self.assertEqual(good.returncode, 0, good.stderr)
                self.assertEqual((root/'stats').read_text(), '1 1 0\n')
                # Without namespace isolation both targets connect: MUST refuse.
                probe = subprocess.run([DRIVER, 'canary-probe', str(config)], capture_output=True, text=True, timeout=5)
                self.assertNotEqual(probe.returncode, 0)
                self.assertIn('connected', probe.stdout)
                # A contact after baseline cannot be mistaken for zero effects.
                again = subprocess.run([DRIVER, 'canary-positive', str(config)], capture_output=True, timeout=5)
                self.assertEqual(again.returncode, 0)
                self.assertNotEqual((root/'stats').read_text(), '1 1 0\n')
            finally:
                (root/'stop').touch()
                try:
                    server.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill(); server.communicate()
            self.assertEqual(server.returncode, 0)

    def test_shutdown_drains_kernel_backlog(self):
        with tempfile.TemporaryDirectory() as td:
            root = pathlib.Path(td)
            server = subprocess.Popen([DRIVER, 'canary-listen', str(root), 'a'*64], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            streams = []
            try:
                for _ in range(100):
                    if (root/'ready').exists() or server.poll() is not None: break
                    time.sleep(.02)
                self.assertTrue((root/'ready').exists())
                subprocess.run([DRIVER, 'canary-positive', str(root/'ready')], check=True, timeout=5)
                # Queue real connections while accept cannot run, then request
                # shutdown. A one-accept-per-loop server loses four of these.
                os.kill(server.pid, signal.SIGSTOP)
                port = int((root/'ready').read_text().split()[0])
                for _ in range(5):
                    stream = socket.create_connection(('127.0.0.1', port), timeout=1)
                    stream.sendall(b'a'*64); streams.append(stream)
                (root/'stop').touch()
                os.kill(server.pid, signal.SIGCONT)
                server.communicate(timeout=5)
                self.assertEqual(server.returncode, 0)
                self.assertEqual((root/'stats').read_text(), '6 1 0\n')
            finally:
                for stream in streams: stream.close()
                if server.poll() is None:
                    os.kill(server.pid, signal.SIGCONT)
                    server.kill(); server.communicate()

    def test_invalid_config_never_connects(self):
        with tempfile.TemporaryDirectory() as td:
            config = pathlib.Path(td)/'config'
            for payload in ('0 12 '+ 'a'*64, '127.0.0.2 12 '+ 'a'*64, '12 13 bad', '12 12 '+ 'a'*64):
                config.write_text(payload)
                result = subprocess.run([DRIVER, 'canary-probe', str(config)], capture_output=True, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(b'connected', result.stdout)

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--driver', required=True)
    args, rest = parser.parse_known_args()
    DRIVER = str(pathlib.Path(args.driver).resolve())
    unittest.main(argv=[__file__, *rest])
