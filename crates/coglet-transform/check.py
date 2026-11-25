#!/usr/bin/env python3
"""Native component proofs and same-run pairing, not publication authority.

CI owns artifact authentication/custody; this parser never accepts a caller's
identity string as production trust. See README for remaining integration gates.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
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
    },
}
REQUIRED_FILES = {"rustc.txt", "Cargo.lock", "build.jsonl", "admission_regressions", "qualification", "admission_regressions.log", "qualification.log", "fuzz.corpus", "fuzz.results"}


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


def compiler_identity(text):
    # Target host differs; compiler version, source revision and LLVM must not.
    return "\n".join(line for line in text.splitlines() if not line.startswith("host:"))


def verify_pair(root, head, run_id, attempt):
    directories = sorted(p for p in root.iterdir() if p.is_dir())
    assert len(directories) == 2, "exactly two architecture artifacts required"
    legs = {}
    for path in directories:
        manifest = json.loads((path / "manifest.json").read_text())
        arch = manifest["arch"]
        assert arch in {"x86_64", "aarch64"} and arch not in legs, "missing or duplicated architecture"
        assert manifest["mode"] == "native"
        assert manifest["actual_checkout"] == head
        assert manifest["github_run"] == run_id and manifest["github_attempt"] == attempt
        assert manifest["tests"] == {key: sorted(value) for key, value in EXPECTED.items()}
        assert set(manifest["files"]) == REQUIRED_FILES
        assert {p.name for p in path.iterdir()} == REQUIRED_FILES | {"manifest.json"}
        for name, expected in manifest["files"].items():
            file = path / name
            assert file.is_file() and not file.is_symlink()
            assert file.stat().st_size > 0 and digest(file) == expected, name
        for name, expected in EXPECTED.items():
            validate_report((path / f"{name}.log").read_text(), expected)
        assert (path / "fuzz.results").stat().st_size == 960 * 80
        legs[arch] = path
    assert set(legs) == {"x86_64", "aarch64"}
    x86, arm = legs["x86_64"], legs["aarch64"]
    for name in ("Cargo.lock", "fuzz.corpus", "fuzz.results"):
        assert (x86 / name).read_bytes() == (arm / name).read_bytes(), name
    assert compiler_identity((x86 / "rustc.txt").read_text()) == compiler_identity((arm / "rustc.txt").read_text())
    print("native x86_64/aarch64: 960 paired WAMR+Wasmtime/oracle cases; component-only")


def native_leg():
    out = Path(os.environ.get("COGLET_TRANSFORM_EVIDENCE_DIR", ROOT / "target/transform-evidence"))
    out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, COGLET_TRANSFORM_EVIDENCE_DIR=str(out))
    arch = run(["uname", "-m"], capture_output=True).stdout.strip()
    assert arch in {"x86_64", "aarch64"}
    run(["git", "diff", "--exit-code", "HEAD", "--", "crates/coglet-transform", "crates/coglet-engine", "crates/coglet-engine-wamr", "crates/coglet-engine-wasmtime", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"])
    head = run(["git", "rev-parse", "HEAD"], capture_output=True).stdout.strip()
    (out / "rustc.txt").write_text(run(["rustc", "-vV"], capture_output=True).stdout)
    shutil.copy2(ROOT / "Cargo.lock", out / "Cargo.lock")
    built = run(["cargo", "test", "--locked", "-p", "coglet-transform", "--test", "admission_regressions", "--test", "qualification", "--no-run", "--message-format=json"], capture_output=True)
    (out / "build.jsonl").write_text(built.stdout)
    bins = {}
    for line in built.stdout.splitlines():
        msg = json.loads(line)
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") and msg.get("target", {}).get("name") in EXPECTED:
            name = msg["target"]["name"]
            assert name not in bins
            bins[name] = Path(msg["executable"])
    assert set(bins) == set(EXPECTED)
    for name, binary in bins.items():
        shutil.copy2(binary, out / name)
        result = subprocess.run([str(binary), "--test-threads=1"], cwd=ROOT, env=env, capture_output=True, text=True)
        (out / f"{name}.log").write_text(result.stdout + result.stderr)
        print(result.stdout + result.stderr, flush=True)
        result.check_returncode()
        validate_report(result.stdout, EXPECTED[name])
    assert {p.name for p in out.iterdir()} == REQUIRED_FILES
    manifest = {
        "scope": "component-only; builder/CI-identity/provenance/CAS not qualified",
        "actual_checkout": head,
        "github_run": env.get("GITHUB_RUN_ID"), "github_attempt": env.get("GITHUB_RUN_ATTEMPT"),
        "arch": arch, "mode": "native",
        "tests": {key: sorted(value) for key, value in EXPECTED.items()},
        "files": {name: digest(out / name) for name in sorted(REQUIRED_FILES)},
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print("Component proof manifest:", out / "manifest.json")


if __name__ == "__main__":
    if sys.argv[1:] == ["--native-leg"]:
        native_leg()
    elif len(sys.argv) == 3 and sys.argv[1] == "--aggregate":
        verify_pair(Path(sys.argv[2]), run(["git", "rev-parse", "HEAD"], capture_output=True).stdout.strip(), os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"])
    elif sys.argv[1:] == ["--aggregate-test"]:
        result = subprocess.run(["cargo", "test", "--locked", "-p", "coglet-transform", "--test", "cross_arch"], cwd=ROOT, capture_output=True, text=True)
        print(result.stdout + result.stderr)
        result.check_returncode()
        validate_report(result.stdout, {"pinned_transform_cross_arch_replay"})
    else:
        raise SystemExit("use --native-leg, --aggregate SAME_RUN_ARTIFACT_DIR, or --aggregate-test")
