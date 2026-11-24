#!/usr/bin/env python3
"""Full frozen ABI inventory, official harness, zero role-filtered skips.

Profile-level response views are selected before starting the implementation;
requests, expectations and timelines are copied byte-for-byte, never rewritten.
Guest-reference exercises the SDK/shared MockEngine, not runtime journaling.
"""
import json
import os
from pathlib import Path
import re
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
    "executable-archive.json": ("runtime", "target/debug/cogletd --mock-stdio --trace-view state"),
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
    print("Archive harness checks are nonexecuting; independent WAMR replay is qualified separately below.", flush=True)
    frozen = len(combined) - len(json.loads((ROOT / "conformance/abi/executable-archive.json").read_text()))
    if frozen != 128:
        raise RuntimeError(f"frozen ABI inventory changed: {frozen} != 128")
    expected_native = {
        "executable_archive_inventory_covers_every_source_record",
        "executable_archive_records_reexecute_on_wamr_with_identical_step_hashes",
        "executable_archive_corruptions_reject_before_guest_entry",
        "executable_archive_replay_rejects_guest_program_mismatch",
        "frozen_archive_corpus_and_spec_v0_remain_unchanged",
    }
    native = subprocess.run(["cargo", "test", "--locked", "-p", "coglet-runtime", "--features", "native-engine",
        "--test", "executable_archive", "--", "--test-threads=1"], cwd=ROOT, text=True, capture_output=True)
    print(native.stdout, end="", flush=True)
    print(native.stderr, end="", file=sys.stderr, flush=True)
    passed = re.findall(r"^test native::([a-z_0-9]+) \.\.\. ok$", native.stdout, re.MULTILINE)
    if native.returncode or set(passed) != expected_native or len(passed) != 5 or "5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out" not in native.stdout:
        raise RuntimeError("native replay qualification missing/failed/skipped")
    report = ET.Element("testsuite", name="executable-archive.native-wamr", tests="5", failures="0", skipped="0")
    for name in passed:
        ET.SubElement(report, "testcase", name=name)
    ET.ElementTree(report).write(output / "native-replay.xml", encoding="utf-8", xml_declaration=True)
    # CI checkout fetch-depth:0 supplies the remote tag without network access
    # during qualification (also compatible with offline dependency-bundle proofs).
    tag = subprocess.check_output(["git", "rev-parse", "--verify", "refs/tags/spec-v0"], cwd=ROOT, text=True).strip()
    if tag != "c348fdeca3c162bdaeef0f65b4da74f5ecfb59f3":
        raise RuntimeError("spec-v0 tag changed or unavailable")
    print("Executable archive replay: all 32 guest records, 5 named tests, 0 failures, 0 skips; spec-v0 unchanged", flush=True)


if __name__ == "__main__":
    main()
