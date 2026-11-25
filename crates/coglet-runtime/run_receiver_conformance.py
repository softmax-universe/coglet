#!/usr/bin/env python3
"""Run the unchanged frozen hub receiver subset against the real DeviceHandler peer."""
from pathlib import Path
import argparse
import os
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from receiver_peer_driver import stop_owned

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
CASES = (
    "action-expired",
    "cross-owner-receipt",
    "unknown-receipt",
    "debug-inbox-protected",
    "debug-traversal",
    "read-grant-is-not-write",
    "write-stale-snapshot",
    "read-actor-isolation",
    "read-ended-ticket",
    "read-missing-ticket",
    "read-scoped-allowed",
    "pause-single-step",
    "single-step-requires-pause",
    "status-actor-isolation",
    "status-ended-ticket",
    "status-missing-ticket",
    "status-scoped-allowed",
    "fetch-actor-isolation",
    "fetch-ended-ticket",
    "fetch-missing-ticket",
    "fetch-scoped-allowed",
    "inbox-boundary-queued",
    "inbox-conflicting-id",
    "inbox-durable-dedup",
    "queue-backpressure",
)
ENV = {
    **os.environ,
    "CARGO_INCREMENTAL": "0",
    "CARGO_PROFILE_DEV_DEBUG": "0",
    "CARGO_PROFILE_TEST_DEBUG": "0",
    "CARGO_BUILD_JOBS": "2",
}


def run(cmd, **kwargs):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, env=ENV, check=True, **kwargs)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--case", choices=CASES, action="append", help="diagnostic subset; not full receiver qualification")
    selected = tuple(parser.parse_args().case or CASES)
    run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "-j",
            "2",
            "-p",
            "coglet-runtime",
            "--features",
            "native-engine",
            "--example",
            "runtime_receiver_peer",
            "--example",
            "qualify_apple_guest",
            "--bin",
            "coglet-runtime-git-validator",
        ]
    )
    guests = ROOT / "target/guests"
    wasm = guests / "wasm32-wasip1/release/coglet_counter.wasm"
    if not wasm.is_file():
        run(["rustup", "target", "add", "wasm32-wasip1"])
        env = {**ENV, "CARGO_TARGET_DIR": str(guests)}
        print("+ cargo rustc counter wasm32-wasip1", flush=True)
        subprocess.run(
            [
                "cargo",
                "rustc",
                "--manifest-path",
                str(ROOT / "coglets/counter/Cargo.toml"),
                "--target",
                "wasm32-wasip1",
                "--release",
                "--lib",
                "--locked",
                "--",
                "-C",
                "strip=symbols",
                "-C",
                "opt-level=s",
                "-C",
                "link-arg=--max-memory=16777216",
            ],
            cwd=ROOT,
            env=env,
            check=True,
        )
        run(
            [
                str(ROOT / "target/release/examples/qualify_apple_guest"),
                str(wasm),
            ]
        )
        ENV["COGLET_COUNTER_WASM"] = str(wasm)
    if wasm.is_file():
        ENV["COGLET_COUNTER_WASM"] = str(wasm)
    report = ROOT / "target/hub-receiver-conformance.xml"
    state = tempfile.mkdtemp(
        prefix="coglet-runtime-receiver-", dir=os.environ.get("TMPDIR", ROOT / "target")
    )
    ENV["COGLET_FIXTURE_RECEIVER_STATE"] = state
    print(f"Receiver oracle evidence: {state}", flush=True)
    try:
        suite = Path(state) / "hub"
        suite.mkdir()
        for case in selected:
            (suite / f"{case}.json").symlink_to(ROOT / f"conformance/hub/{case}.json")
        run(
            [
                str(ROOT / "conformance/run"),
                "--registry",
                str(HERE / "hub-receiver-conformance.toml"),
                "--cases",
                state,
                "--junit",
                str(report),
            ]
        )
    finally:
        for custody in Path(state).glob("*.custody.json"):
            stop_owned(custody)
    cases = ET.parse(report).findall(".//testcase")
    names = sorted(c.get("name") or "" for c in cases)
    if names != sorted(selected) or any(
        c.find("skipped") is not None
        or c.find("failure") is not None
        or c.find("error") is not None
        for c in cases
    ):
        raise SystemExit(
            f"incomplete receiver subset {names}; skips are not passes"
        )
    from check_receiver_evidence import check
    print(check(state, selected))
    print(
        f"Receiver subset: {len(cases)} unchanged hub cases passed; zero skipped. "
        "Full hub conformance is not claimed."
    )
