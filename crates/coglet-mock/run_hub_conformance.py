#!/usr/bin/env python3
"""Run every unchanged hub enrollment/session case against the real-iroh mock."""
from pathlib import Path
import os
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
CASES = (
    "enroll-reconnect", "health-minimal", "enroll-unsupported-version",
    "enroll-one-use", "enroll-expired", "enroll-window-closed",
    "enroll-wrong-node", "start-before-enroll",
)

if __name__ == "__main__":
    subprocess.run(["cargo", "build", "--locked", "-j", "2", "-p", "coglet-mock", "--features", "test-controls", "--example", "mock_hub_peer"], cwd=ROOT, check=True)
    report = ROOT / "target/mock-hub-conformance.xml"
    with tempfile.TemporaryDirectory(prefix="coglet-mock-hub-", dir=os.environ.get("TMPDIR", ROOT / "target")) as state:
        suite = Path(state) / "hub"
        suite.mkdir()
        for case in CASES:
            (suite / f"{case}.json").symlink_to(ROOT / f"conformance/hub/{case}.json")
        subprocess.run([str(ROOT / "conformance/run"), "--registry", str(HERE / "hub-conformance.toml"), "--cases", state, "--junit", str(report)], cwd=ROOT, check=True)
    cases = ET.parse(report).findall(".//testcase")
    if len(cases) != len(CASES) or any(c.find(tag) is not None for c in cases for tag in ("skipped", "failure", "error")):
        raise SystemExit("incomplete advertised mock hub subset")
    print(f"Mock hub enrollment/session: {len(cases)} unchanged cases passed; zero skipped. Not full Git/lifecycle hub conformance.")
