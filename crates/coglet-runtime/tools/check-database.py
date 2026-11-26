#!/usr/bin/env python3
"""Actual real/mock DB runtime qualification, plus the frozen wire corpus.

No expected response is passed to the adapter. Fresh JUnit inventory must match
ALL cases, including any added case; missing/skipped cases never count as green.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def registry_text(engine):
    """Generated ADR-0041 registry: real/mock adapter, functional watchdog opted in."""
    command = [sys.executable, "conformance/engine/adapter.py", "target/debug/examples/database_adapter", engine]
    return "\n".join([
        "[[implementations]]",
        f"name = {json.dumps(f'database-{engine}')}",
        'suites = ["database"]',
        'transport = "stdio"',
        f"command = {json.dumps(command)}",
        'peer_roles = ["guest"]',
        "database_functional_watchdog = true",
        "",
    ])


def check_watchdog():
    """Qualification controls shared by the required DB entrypoint and focused local runs."""
    # Keep registry/prelaunch/transport negatives in the existing required DB lane.
    run(sys.executable, "crates/coglet-runtime/tools/test_database_watchdog.py")
    with tempfile.TemporaryDirectory(prefix="coglet-db-watchdog-") as temporary:
        evidence = Path(temporary) / "proof"
        try:
            run(sys.executable, "crates/coglet-runtime/tools/prove_database_watchdog.py",
                "--evidence", str(evidence))
        finally:
            # Temporary proof bytes are not an uploaded artifact; retain raw diagnostics in CI stdout.
            for name in ("metadata.json", "red.log", "green.log", "results.json"):
                path = evidence / name
                if path.exists():
                    print(f"database watchdog {name}:\n{path.read_text()}", flush=True)


def main():
    run("rustup", "target", "add", "wasm32-unknown-unknown")
    run("cargo", "test", "--locked", "-p", "coglet-db")
    run("cargo", "test", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--", "--test-threads=1")
    run("cargo", "clippy", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--all-targets", "--", "-D", "warnings")
    run("cargo", "build", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--example", "database_adapter")
    check_watchdog()
    cases = json.loads((ROOT / "conformance/database/cases.json").read_text())
    expected = {case["name"] for case in cases}
    assert len(expected) == len(cases), "duplicate database case names"
    for engine in ("real", "mock"):
        report = ROOT / f"target/database-{engine}.xml"
        report.unlink(missing_ok=True)
        registry = ROOT / f"target/database-{engine}-impls.toml"
        registry.write_text(registry_text(engine))
        run(sys.executable, "conformance/run", "--registry", str(registry), "--junit", str(report))
        suite = ET.parse(report).getroot()
        names = [c.attrib["name"] for c in suite.findall("testcase")]
        assert len(names) == len(expected) and set(names) == expected, names
        assert suite.attrib["failures"] == "0" and suite.attrib["skipped"] == "0", suite.attrib
        print(f"{engine}: verified {len(names)} named frozen DB cases, zero failures/skips", flush=True)


if __name__ == "__main__":
    main()
