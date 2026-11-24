#!/usr/bin/env python3
"""Actual real/mock DB runtime qualification, plus the frozen wire corpus.

No expected response is passed to the adapter. Fresh JUnit inventory must match
ALL cases, including any added case; missing/skipped cases never count as green.
"""
import json
from pathlib import Path
import shlex
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def main():
    run("rustup", "target", "add", "wasm32-unknown-unknown")
    run("cargo", "test", "--locked", "-p", "coglet-db")
    run("cargo", "test", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--", "--test-threads=1")
    run("cargo", "clippy", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--all-targets", "--", "-D", "warnings")
    run("cargo", "build", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--example", "database_adapter")
    cases = json.loads((ROOT / "conformance/database/cases.json").read_text())
    expected = {case["name"] for case in cases}
    assert len(expected) == len(cases), "duplicate database case names"
    for engine in ("real", "mock"):
        report = ROOT / f"target/database-{engine}.xml"
        report.unlink(missing_ok=True)
        command = shlex.join([sys.executable, "conformance/engine/adapter.py", "target/debug/examples/database_adapter", engine])
        run(sys.executable, "conformance/run", "--impl", command, "--suite", "database", "--peer-role", "guest", "--junit", str(report))
        suite = ET.parse(report).getroot()
        names = [c.attrib["name"] for c in suite.findall("testcase")]
        assert len(names) == len(expected) and set(names) == expected, names
        assert suite.attrib["failures"] == "0" and suite.attrib["skipped"] == "0", suite.attrib
        print(f"{engine}: verified {len(names)} named frozen DB cases, zero failures/skips", flush=True)


if __name__ == "__main__":
    main()
