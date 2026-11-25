#!/usr/bin/env python3
"""Real daemon tests; no mock daemon and no guest/action callback."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "conformance"))
from wire import encode, decode

BRIDGE = Path(__file__).with_name("profile_fixture_bridge.py")
DAEMON = Path(os.environ.get("COGLET_FIXTURE_COGLETD", ROOT / "target/debug/cogletd"))
S = "surface:" + "b" * 64
C = "coglet:" + "a" * 64


def read_exact(fd, count):
    out = bytearray()
    deadline = time.monotonic() + 5
    while len(out) < count:
        if not select.select([fd], [], [], max(0, deadline - time.monotonic()))[0]:
            raise AssertionError("timed out reading daemon bytes")
        part = os.read(fd, count - len(out))
        if not part:
            raise AssertionError("unexpected EOF")
        out.extend(part)
    return bytes(out)


def frame(raw):
    return struct.pack(">I", len(raw)) + raw


class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="bt-", dir=os.environ["TMPDIR"])
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        cases = json.loads((ROOT / "conformance/abi/cases.json").read_text())
        setup = next(c for c in cases if c["name"] == "no-effects")["steps"][0]["send"]
        setup["manifest"]["id"] = C
        self.setup = self.dir / "setup.json"
        self.setup.write_text(json.dumps(setup))
        self.binding = self.dir / "binding.json"
        self.binding.write_text(json.dumps({"surface": S, "permitted_coglets": [C]}))
        self.catalog = ROOT / "crates/coglet-runtime/examples/trusted-extension-catalog.json"
        self.env = dict(os.environ, TMPDIR=str(self.dir),
            COGLET_FIXTURE_COGLETD=str(DAEMON.resolve()),
            COGLET_FIXTURE_CONFIG=str(ROOT / "spec/fixtures/surface/adapter-config.json"),
            COGLET_FIXTURE_RUNTIME_SETUP=str(self.setup),
            COGLET_FIXTURE_RUNTIME_BINDING=str(self.binding),
            COGLET_FIXTURE_CATALOG=str(self.catalog),
            COGLET_FIXTURE_CATALOG_SHA256=hashlib.sha256(self.catalog.read_bytes()).hexdigest())
        self.env.pop("COGLET_CONFORMANCE_CONTRACT_PROFILE", None)

    def launch(self, profile=None):
        env = dict(self.env)
        if profile is not None:
            env["COGLET_CONFORMANCE_CONTRACT_PROFILE"] = profile
        p = subprocess.Popen([sys.executable, str(BRIDGE)], env=env,
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        def cleanup():
            if p.poll() is None:
                p.terminate()
                p.wait(timeout=5)
            for f in (p.stdin, p.stdout, p.stderr):
                f.close()
        self.addCleanup(cleanup)
        return p

    def ready(self, p):
        line = bytearray()
        deadline = time.monotonic() + 21
        while time.monotonic() < deadline:
            line.extend(read_exact(p.stderr.fileno(), 1))
            if line.endswith(b"\n"):
                if line == b"COGLET_CONFORMANCE_READY\n":
                    return
                line.clear()
        self.fail("no authenticated readiness")

    def handshake(self, profile, selected):
        p = self.launch(profile)
        self.ready(p)
        # Actual child PID is independently observed, not inferred from readiness.
        child_ids = Path(f"/proc/{p.pid}/task/{p.pid}/children").read_text().split()
        self.assertEqual(len(child_ids), 1)
        child_pid = int(child_ids[0])
        private = list(self.dir.glob("cf-*"))
        self.assertEqual(len(private), 1)
        self.assertEqual(private[0].stat().st_mode & 0o777, 0o700)
        self.assertTrue((private[0] / "w").exists())
        self.assertFalse(select.select([p.stdout], [], [], .05)[0], "fabricated unsolicited output")
        hello = encode({"type": "hello", "body": {"surface": S, "clock": 1000,
            "offer": {"versions": ["0.1", "0.2"]}, "correlation_id": "1" * 32}})
        wire = frame(hello)
        # Deliberately split both the length prefix and the CBOR payload.
        for start, end in ((0, 1), (1, 3), (3, 9), (9, len(wire))):
            p.stdin.write(wire[start:end])
            p.stdin.flush()
        header = read_exact(p.stdout.fileno(), 4)
        payload = read_exact(p.stdout.fileno(), struct.unpack(">I", header)[0])
        expected = encode({"type": "hello_reply", "body": {
            "selection": {"version": selected}, "correlation_id": "1" * 32}})
        self.assertEqual(header + payload, frame(expected))
        self.assertEqual(decode(payload)["body"]["selection"]["version"], selected)
        # Independently run real production socket mode and compare exact bytes.
        direct_path = self.dir / "d"
        child = subprocess.Popen([str(DAEMON), "--surface-service-socket", str(direct_path),
            str(self.binding), str(os.geteuid()), str(self.dir / "direct.wal"), str(self.setup),
            "--contract-profile", profile or "surface-0.1", "--trusted-extension-catalog",
            str(self.catalog), self.env["COGLET_FIXTURE_CATALOG_SHA256"]],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not direct_path.exists():
                self.assertIsNone(child.poll())
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            with socket.socket(socket.AF_UNIX) as link:
                link.connect(str(direct_path))
                link.sendall(wire)
                self.assertEqual(read_exact(link.fileno(), len(header + payload)), header + payload)
        finally:
            child.terminate()
            child.wait(timeout=5)
            child.stderr.close()
        p.stdin.close()
        self.assertEqual(p.wait(timeout=5), 0)
        self.assertFalse(Path(f"/proc/{child_pid}").exists(), "daemon leaked after EOF")
        self.assertFalse(private[0].exists(), "private socket/WAL directory leaked")

    def test_selected_02_exact_bytes(self):
        self.handshake("surface-0.2", "0.2")

    def test_default_selected_01_exact_bytes(self):
        self.handshake(None, "0.1")

    def test_wrong_pin_refused_without_ready_or_response(self):
        self.env["COGLET_FIXTURE_CATALOG_SHA256"] = "0" * 64
        p = self.launch("surface-0.2")
        stdout, stderr = p.communicate(timeout=5)
        self.assertNotEqual(p.returncode, 0)
        self.assertNotIn(b"COGLET_CONFORMANCE_READY", stderr)
        self.assertIn(b"pin mismatch", stderr)
        self.assertEqual(stdout, b"")
        self.assertFalse(list(self.dir.glob("cf-*")))

    def test_bad_selectors_fail_closed(self):
        for key, value in (("COGLET_CONFORMANCE_CONTRACT_PROFILE", "0.2"),
                           ("COGLET_FIXTURE_CATALOG_SHA256", "BAD")):
            with self.subTest(key=key):
                env = dict(self.env, **{key: value})
                done = subprocess.run([sys.executable, str(BRIDGE)], env=env,
                                      input=b"", capture_output=True, timeout=5)
                self.assertNotEqual(done.returncode, 0)
                self.assertNotIn(b"COGLET_CONFORMANCE_READY", done.stderr)
                self.assertEqual(done.stdout, b"")

    def test_disconnect_has_no_fabricated_response(self):
        p = self.launch("surface-0.2")
        self.ready(p)
        child = int(Path(f"/proc/{p.pid}/task/{p.pid}/children").read_text().strip())
        # This is invalid CBOR: the production receiver, not this bridge, rejects it.
        p.stdin.write(frame(b"\xff"))
        p.stdin.flush()
        self.assertEqual(p.wait(timeout=5), 0)
        self.assertEqual(p.stdout.read(), b"")
        self.assertFalse(Path(f"/proc/{child}").exists())
        self.assertFalse(list(self.dir.glob("cf-*")))

    def test_sigterm_reaps_actual_daemon(self):
        p = self.launch("surface-0.2")
        self.ready(p)
        child = int(Path(f"/proc/{p.pid}/task/{p.pid}/children").read_text().strip())
        p.terminate()
        p.wait(timeout=5)
        self.assertFalse(Path(f"/proc/{child}").exists())
        self.assertFalse(list(self.dir.glob("cf-*")))


if __name__ == "__main__":
    unittest.main(verbosity=2)
