#!/usr/bin/env python3
"""Full frozen ABI inventory, official harness, zero role-filtered skips.

Profile-level response views are selected before starting the implementation;
requests, expectations and timelines are copied byte-for-byte, never rewritten.
Guest-reference exercises the SDK/shared MockEngine, not runtime journaling.
"""
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
PROFILES = {
    "cases.json": ("runtime", "target/debug/cogletd --mock-stdio"),
    "admission.json": ("runtime", "target/debug/cogletd --mock-stdio"),
    "manifest-admission.json": ("runtime", "target/debug/cogletd --mock-stdio"),
    "boundary-receipts.json": ("runtime", "target/debug/cogletd --mock-stdio --trace-view state"),
    "heard-archive.json": ("runtime", "target/debug/cogletd --mock-stdio --trace-view state"),
    "provenance.json": ("runtime", "target/debug/cogletd --mock-stdio --trace-view validity"),
    "reference-guests.json": ("guest-reference", "python3 conformance/engine/adapter.py target/debug/examples/reference_conformance mock"),
}


# These archive fixtures are fully materialized in their send.trace. No live
# device, grant, or provider setup is needed; each peer is a fresh process. The
# explicit allow-list refuses any future fixture with additional prerequisites.
ARCHIVE_FIXTURES = {"abi." + name for name in '''
heard-export-replay-journal-alone heard-before-reasoning-step
heard-continuation-initial-state heard-late-measured-alignment heard-utf8-prefix-cap
heard-unheard-prefix heard-queued-text heard-no-alignment-text
heard-played-past-duration heard-duplicate-item heard-alignment-order
heard-alignment-past-duration heard-unknown-state-field heard-unknown-item-field
heard-position-regression heard-stopped-advance heard-stopped-revive
heard-alignment-rewrite heard-item-removal heard-hub-forgery heard-actor-forgery
heard-ticket-forgery heard-wrong-path heard-deletion heard-guest-write
heard-guest-create
'''.split()}


def main():
    discovered = {str(p.relative_to(ROOT / "conformance/abi")) for p in (ROOT / "conformance/abi").rglob("*.json")}
    if discovered != set(PROFILES):
        raise RuntimeError(f"unrouted/missing ABI profiles: {discovered ^ set(PROFILES)}")
    subprocess.run(["cargo", "build", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--bin", "cogletd"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "coglet-guest-rust", "--example", "reference_conformance"], cwd=ROOT, check=True)
    output = ROOT / "target/runtime-all-abi"
    output.mkdir(parents=True, exist_ok=True)
    combined = ET.Element("testsuite", name="runtime-and-shared-mock.full-abi")
    all_expected = []
    for filename, (role, command) in PROFILES.items():
        source = ROOT / "conformance/abi" / filename
        # Names/roles are report inventory only; no expected result is consumed.
        inventory = json.loads(source.read_text())
        expected = [case["name"] for case in inventory]
        if len(expected) != len(set(expected)) or any(case.get("peer_role", "runtime") != role for case in inventory):
            raise RuntimeError(f"invalid inventory/role in {filename}")
        with tempfile.TemporaryDirectory(prefix="runtime-abi-", dir=os.environ.get("TMPDIR", ROOT / "target")) as directory:
            cases = Path(directory)
            (cases / "abi").mkdir()
            shutil.copyfile(source, cases / "abi" / filename)
            report = cases / "fresh.xml"
            labels = {label for case in inventory for label in case.get("fixtures", [])}
            if labels - ARCHIVE_FIXTURES:
                raise RuntimeError(f"unsupported fixture requirements: {labels - ARCHIVE_FIXTURES}")
            registry = cases / "impl.toml"
            config = ('[[implementations]]\nname = "runtime-and-shared-mock"\nsuites = ["abi"]\ntransport = "stdio"\n'
                      + 'peer_roles = ' + json.dumps([role]) + '\ncommand = ' + json.dumps(shlex.split(command)) + '\n')
            for label in sorted(labels):
                config += '\n[implementations.fixtures.' + json.dumps(label) + ']\n'
            registry.write_text(config)
            result = subprocess.run([sys.executable, str(ROOT / "conformance/run"), "--registry", str(registry), "--cases", str(cases),
                "--junit", str(report)], cwd=ROOT)
            suite = ET.parse(report).getroot()
            actual = suite.findall("testcase")
            names = [case.attrib["name"] for case in actual]
            shutil.copyfile(report, output / (source.stem + ".xml"))
            if result.returncode or sorted(names) != sorted(expected) or any(case.find("failure") is not None or case.find("skipped") is not None for case in actual):
                raise RuntimeError(f"ABI profile failed/incomplete/skipped: {filename}")
            if suite.attrib["failures"] != "0" or suite.attrib["skipped"] != "0":
                raise RuntimeError(f"non-green suite: {filename}")
            combined.extend(actual)
            all_expected.extend(expected)
    if len(all_expected) != len(set(all_expected)) or len(combined) != len(all_expected):
        raise RuntimeError("full-suite inventory collision/incomplete aggregate")
    combined.set("tests", str(len(combined)))
    combined.set("failures", "0")
    combined.set("skipped", "0")
    ET.ElementTree(combined).write(output / "all.xml", encoding="utf-8", xml_declaration=True)
    print(f"Full ABI inventory: {len(combined)} passed, 0 failures, 0 skips; {output / 'all.xml'}", flush=True)
    print("Runtime scripted steps/archive checks + actual WAMR admission + SDK shared native mock. NOT native guest-step, authentication, or independent all-profile replay qualification.", flush=True)


if __name__ == "__main__":
    main()
