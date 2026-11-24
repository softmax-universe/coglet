#!/usr/bin/env python3
"""Required component-proof runner. Not a publication qualification authority.

Bypasses the workspace quarantine wrapper and fails if any declared test is
missing, renamed, ignored, filtered, duplicated or failed. Retains exact corpus,
results, executed test binaries, and environment/source pins for later review.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(os.environ.get("COGLET_TRANSFORM_EVIDENCE_DIR", ROOT / "target/transform-evidence"))
EXPECTED = {
    "admission_regressions": {
        "rewrite_refuses_excessive_locals_before_expansion",
        "wasi_wrong_signature_is_refused_by_rewrite",
        "signature_preimage_has_unambiguous_field_boundaries",
        "publication_rejects_unbound_evidence_and_profile_pins",
        "all_qualification_pins_are_required",
        "local_admission_signature_authority_expiry_revocation_and_ref_binding",
        "unqualified_transform_publication_refused",
    },
    "qualification": {
        "rewrite_component_changes_arithmetic_and_preserves_identity",
        "pinned_transform_fuzz_differential",
        "pinned_transform_cross_arch_replay",
    },
}


def run(args, **kw):
    return subprocess.run(args, cwd=ROOT, check=True, text=True, **kw)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_report(text, expected):
    rows = re.findall(r"^test (\w+) \.\.\. (\w+)$", text, re.M)
    assert len(rows) == len(expected), (rows, expected)
    assert {name for name, _ in rows} == expected, (rows, expected)
    assert all(result == "ok" for _, result in rows), rows
    summaries = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out", text)
    assert summaries == [(str(len(expected)), "0", "0", "0", "0")], summaries


def main():
    # The evidence directory is a new invocation's private output, never recycled.
    OUT.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, COGLET_TRANSFORM_EVIDENCE_DIR=str(OUT))
    native_arch = run(["uname", "-m"], capture_output=True).stdout.strip()
    assert native_arch == "x86_64", native_arch
    assert env["COGLET_TRANSFORM_PEER_ARCH"] == "aarch64"
    assert env["COGLET_TRANSFORM_PEER_MODE"] == "emulated-qemu"
    run(["git", "diff", "--exit-code", "HEAD", "--", "crates/coglet-transform", "crates/coglet-engine", "crates/coglet-engine-wamr", "crates/coglet-engine-wasmtime", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"])
    head = run(["git", "rev-parse", "HEAD"], capture_output=True).stdout.strip()
    toolchain = run(["rustc", "-vV"], capture_output=True).stdout
    (OUT / "rustc.txt").write_text(toolchain)
    shutil.copy2(ROOT / "Cargo.lock", OUT / "Cargo.lock")
    # Cargo tells us the actual executables; do not guess hashed target paths.
    built = run(["cargo", "test", "--locked", "-p", "coglet-transform", "--test", "admission_regressions", "--test", "qualification", "--no-run", "--message-format=json"], capture_output=True)
    (OUT / "build.jsonl").write_text(built.stdout)
    bins = {}
    for line in built.stdout.splitlines():
        msg = json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") and msg.get("target", {}).get("name") in EXPECTED:
            name = msg["target"]["name"]
            assert name not in bins, name
            bins[name] = Path(msg["executable"])
    assert set(bins) == set(EXPECTED), bins
    for name, binary in bins.items():
        shutil.copy2(binary, OUT / name)
        result = subprocess.run([str(binary), "--test-threads=1"], cwd=ROOT, env=env, capture_output=True, text=True)
        (OUT / f"{name}.log").write_text(result.stdout + result.stderr)
        print(result.stdout + result.stderr, flush=True)
        result.check_returncode()
        validate_report(result.stdout, EXPECTED[name])
    for kind in ("fuzz", "cross-arch"):
        assert (OUT / f"{kind}.corpus").stat().st_size > 0
        assert (OUT / f"{kind}.results").stat().st_size > 0
    assert (OUT / "fuzz.corpus").read_bytes() == (OUT / "cross-arch.corpus").read_bytes()
    shutil.copy2(ROOT / "target/aarch64-unknown-linux-gnu/debug/coglet-transform-peer", OUT / "peer-aarch64")
    qemu = shutil.which("qemu-aarch64-static")
    assert qemu is not None, "missing executed emulator"
    shutil.copy2(qemu, OUT / "qemu-aarch64-static")
    manifest = {
        "scope": "component-only; builder/CI-identity/provenance/CAS not qualified",
        "actual_checkout": head,
        "github_head": env.get("GITHUB_SHA"),
        "github_run": env.get("GITHUB_RUN_ID"),
        "github_attempt": env.get("GITHUB_RUN_ATTEMPT"),
        "modes": ["x86_64/native/wamr-interpreter+wasmtime", "aarch64/emulated-qemu/wamr-interpreter+wasmtime"],
        "tests": {key: sorted(value) for key, value in EXPECTED.items()},
        "files": {p.name: digest(p) for p in sorted(OUT.iterdir()) if p.is_file()},
    }
    (OUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print("Component proof manifest:", OUT / "manifest.json")


if __name__ == "__main__":
    main()
