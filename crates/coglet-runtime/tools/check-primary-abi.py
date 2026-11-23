#!/usr/bin/env python3
"""Run the unmodified primary ABI corpus through the official black-box harness.

This is deliberately a SUBSET gate: the other ABI profiles remain required.
No expected values are read by the executable or rewritten by this launcher.
"""
import os
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parents[3]
subprocess.run(["cargo", "build", "--locked", "-p", "coglet-runtime", "--bin", "cogletd"], cwd=root, check=True)
with tempfile.TemporaryDirectory(prefix="runtime-primary-abi-", dir=os.environ.get("TMPDIR", root / "target")) as directory:
    cases = Path(directory)
    (cases / "abi").mkdir()
    shutil.copyfile(root / "conformance/abi/cases.json", cases / "abi/cases.json")
    print("SUBSET ONLY: conformance/abi/cases.json; other ABI profiles are not qualified", flush=True)
    report = cases / "results.xml"
    result = subprocess.run([sys.executable, str(root / "conformance/run"),
        "--impl", "target/debug/cogletd --mock-stdio", "--suite", "abi",
        "--cases", str(cases), "--junit", str(report)], cwd=root)
    # Empty/no-op runner invocations and stale reports must never look green.
    suite = ET.parse(report).getroot()
    expected = {case["name"] for case in json.loads((cases / "abi/cases.json").read_text())}
    actual = [case.attrib["name"] for case in suite.findall("testcase")]
    if result.returncode == 0 and (len(actual) != len(expected) or set(actual) != expected
            or suite.attrib["failures"] != "0" or suite.attrib["skipped"] != "0"):
        raise RuntimeError("harness report is incomplete or contains failures/skips")
    shutil.copyfile(report, root / "target/runtime-primary-abi.xml")
    sys.exit(result.returncode)
