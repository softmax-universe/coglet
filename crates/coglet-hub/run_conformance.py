#!/usr/bin/env python3
"""Run the unchanged frozen hub event kernel subset, with zero skips allowed."""
from pathlib import Path
import os
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
CASES = (
    "event-batch-dedup", "event-coglet-forged-event", "event-empty-probe",
    "event-foreign-coglet", "event-gap", "event-offset-conflict",
    "event-retry-a-after-b", "event-runtime-wake", "event-source-gap",
    "event-source-repeat",
)
if __name__ == "__main__":
    subprocess.run(["cargo", "build", "--locked", "-j", "2", "-p", "coglet-hub", "--example", "conformance_peer"], cwd=ROOT, check=True)
    report = ROOT / "target/hub-kernel-conformance.xml"
    with tempfile.TemporaryDirectory(prefix="coglet-hub-conformance-", dir=os.environ.get("TMPDIR", ROOT / "target")) as state:
        suite = Path(state) / "hub"
        suite.mkdir()
        for case in CASES:
            # Symlink the exact original bytes. No fixture rewriting or alternative cases.
            (suite / f"{case}.json").symlink_to(ROOT / f"conformance/hub/{case}.json")
        subprocess.run([str(ROOT / "conformance/run"), "--registry", str(HERE / "conformance.toml"), "--cases", state, "--junit", str(report)], cwd=ROOT, check=True)
    cases = ET.parse(report).findall(".//testcase")
    if len(cases) != len(CASES) or any(c.find("skipped") is not None or c.find("failure") is not None or c.find("error") is not None for c in cases):
        raise SystemExit("incomplete kernel subset; skips are not passes")
    print(f"Kernel subset: {len(cases)} unchanged hub cases passed; zero skipped. Full hub conformance is not claimed.")
