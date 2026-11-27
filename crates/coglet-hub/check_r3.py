#!/usr/bin/env python3
"""Required H-transform publication/runtime inventory and same-run pairing.

No quarantine, filtering, retries, synthetic credentials or zero-test passes.
The live producer authenticates a protected completed GitHub run. Consumers
execute its one admitted archive unchanged on two native architectures.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import traceback

ROOT = Path(__file__).resolve().parents[2]
BUILD_TESTS = {
    "artifact_and_audit_rollback_together_then_exact_retry_succeeds",
    "artifact_capacity_refuses_new_custody_without_evicting_retained_bytes",
    "delegation_revocation_during_validation_is_rechecked_before_router_signing",
    "digest_addressed_build_evidence_roundtrip",
    "expired_unknown_and_revoked_delegations_never_retain_signed_output",
    "final_publication_authority_is_checked_inside_retention_transaction",
    "key_isolation_source_scripts_and_repository_hooks_cannot_run",
    "pinned_float_simd_and_ambient_import_sources_remain_unqualified",
    "r3::authenticated_transform_publication_admission",
    "r3::live::protected_r3_configuration_publishes_from_real_ci",
    "r3::r3_authority_races_and_retention_failure_leave_no_signed_artifact",
    "r3::r3_output_gate_failure_and_changed_admission_issuer_refused",
    "r3::rewrite_before_build_commit",
    "r3::unqualified_transform_builder_refused",
    "real_pinned_integer_fixture_compiles_with_exact_provenance",
    "retained_artifact_cannot_be_replaced_with_sql_replace",
    "tampered_pinned_source_and_manifest_are_denied_before_compilation",
    "unqualified_profile_and_wrong_output_gate_pin_are_denied",
    "verified_fixture_is_signed_and_restart_returns_identical_artifact",
}
RUNTIME_TESTS = {"published_r3_runtime_journal_fs_trap_payload_chain"}
RUNTIME_FILES = {"runtime.json", "records.json", "runtime.log", "r3_runtime", "r3_runtime.build.jsonl", "rustc.txt", "Cargo.lock"}
PUBLICATION_FILES = {"publication.json", "publication.bundle", "step.wasm", "runtime-setup.json", "build.log"}
APPROVED_ENGINE = json.loads((Path(__file__).parent / "tests/build/qualified-ci.json").read_text())["runtime_engine"]


def publication_files(root):
    artifact = json.loads((root / "publication.json").read_text())
    pins = set(artifact["reproduction"]["evidence"]["blobs"].values())
    assert pins and all(re.fullmatch(r"[a-f0-9]{64}", pin) for pin in pins)
    for pin in pins:
        assert digest(root / "evidence" / pin) == pin
    return PUBLICATION_FILES | {f"evidence/{pin}" for pin in pins} | {"build", "build.build.jsonl"}


def inventory(root):
    assert not any(p.is_symlink() for p in root.rglob("*"))
    return {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_report(text, expected):
    rows = re.findall(r"^test ([\w:]+) \.\.\. (\w+)$", text, re.M)
    assert len(rows) == len(expected) and {n for n, _ in rows} == expected, rows
    assert all(outcome == "ok" for _, outcome in rows), rows
    summaries = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out", text)
    assert summaries == [(str(len(expected)), "0", "0", "0", "0")], summaries


def identity():
    subprocess.run(["git", "diff", "--exit-code", "HEAD"], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    return head, os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"]


def seal(out, expected):
    assert inventory(out) == expected
    head, run_id, attempt = identity()
    manifest = {"head": head, "run_id": run_id, "run_attempt": attempt,
                "arch": platform.machine(), "mode": "native",
                "files": {n: digest(out / n) for n in sorted(expected)}}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def verify_manifest(path, expected, head, run_id, attempt):
    assert inventory(path) == expected | {"manifest.json"}
    manifest = json.loads((path / "manifest.json").read_text())
    assert manifest["head"] == head
    assert manifest["run_id"] == run_id and manifest["run_attempt"] == attempt
    assert manifest["mode"] == "native"
    assert set(manifest["files"]) == expected
    for name, pin in manifest["files"].items():
        file = path / name
        assert file.is_file() and not file.is_symlink() and file.stat().st_size > 0
        assert digest(file) == pin, name
    return manifest


def verify_runtime_pair(root, head, run_id, attempt):
    dirs = sorted(p for p in root.iterdir() if p.is_dir())
    assert len(dirs) == 2, "both native replay architectures are mandatory"
    legs = {}
    raw = {}
    for path in dirs:
        m = verify_manifest(path, RUNTIME_FILES, head, run_id, attempt)
        arch = m["arch"]
        assert arch in {"x86_64", "aarch64"} and arch not in legs
        validate_report((path / "runtime.log").read_text(), RUNTIME_TESTS)
        r = json.loads((path / "runtime.json").read_text())
        records = json.loads((path / "records.json").read_text())
        assert r["arch"] == arch and r["mode"] == "native" and r["cases"] == 3
        assert r["statuses"] == ["committed", "committed", "aborted"]
        assert len(r["state_hashes"]) == 3 and r["replay_hashes"] == r["state_hashes"]
        assert r["state_hashes"][0] != r["state_hashes"][1] == r["state_hashes"][2]
        assert len(records) == 3
        assert [rec["status"] for rec in records] == r["statuses"]
        assert [rec["output_hash"] for rec in records] == r["state_hashes"]
        assert all(rec["engine"] == APPROVED_ENGINE for rec in records)
        assert records[2]["failure"]["kind"] == "trap"
        raw[arch] = [{key: rec[key] for key in ("engine", "calls", "seed", "logical_time", "events", "input_hash", "output_hash", "code_hash", "manifest_hash")} for rec in records]
        for pin in [r["wasm_sha256"], r["setup_sha256"], r["publication_sha256"], r["initial_hash"], *r["state_hashes"]]:
            assert re.fullmatch(r"[a-f0-9]{64}", pin), pin
        legs[arch] = r
    assert set(legs) == {"x86_64", "aarch64"}
    assert raw["x86_64"] == raw["aarch64"], "raw journal calls/inputs/engine must match, including rolled-back trap observations"
    paths = {json.loads((p / "manifest.json").read_text())["arch"]: p for p in dirs}
    assert (paths["x86_64"] / "Cargo.lock").read_bytes() == (paths["aarch64"] / "Cargo.lock").read_bytes()
    compiler = lambda p: "\n".join(line for line in (p / "rustc.txt").read_text().splitlines() if not line.startswith("host:"))
    assert compiler(paths["x86_64"]) == compiler(paths["aarch64"])
    left, right = legs["x86_64"], legs["aarch64"]
    for key in ("wasm_sha256", "setup_sha256", "publication_sha256", "initial_hash", "state_hashes", "replay_hashes", "statuses", "cases"):
        assert left[key] == right[key], key
    print("native x86_64/aarch64: identical admitted output, initial FS and raw per-step state hashes; 2 commits + 1 rolled-back trap, replay exact")
    return left


def run_inventory(target, log, expected):
    built = subprocess.run(["cargo", "test", "--locked", "-p", "coglet-hub", "--features", "r3-qualification", "--test", target, "--no-run", "--message-format=json"], cwd=ROOT, capture_output=True, text=True)
    print(built.stderr, flush=True)
    built.check_returncode()
    bins = [m["executable"] for line in built.stdout.splitlines()
            if (m := json.loads(line)).get("reason") == "compiler-artifact"
            and m.get("target", {}).get("name") == target and m.get("executable")]
    assert len(bins) == 1, bins
    shutil.copy2(bins[0], log.parent / target)
    (log.parent / f"{target}.build.jsonl").write_text(built.stdout)
    result = subprocess.run([bins[0], "--test-threads=1"], cwd=ROOT, capture_output=True, text=True)
    log.write_text(result.stdout + result.stderr)
    print(result.stdout + result.stderr, flush=True)
    result.check_returncode()
    validate_report(result.stdout, expected)


def publication():
    assert platform.system() == "Linux" and platform.machine() == "x86_64"
    out = Path(os.environ["COGLET_R3_PUBLICATION_DIR"])
    out.mkdir(parents=True, exist_ok=False)
    subprocess.run(["cargo", "build", "--locked", "-p", "coglet-transform", "--bins"], cwd=ROOT, check=True)
    run_inventory("build", out / "build.log", BUILD_TESTS)
    seal(out, publication_files(out))


def runtime():
    assert platform.system() == "Linux" and platform.machine() in {"x86_64", "aarch64"}
    source = Path(os.environ["COGLET_R3_PUBLICATION_DIR"])
    head, run_id, attempt = identity()
    verify_manifest(source, publication_files(source), head, run_id, attempt)
    validate_report((source / "build.log").read_text(), BUILD_TESTS)
    out = Path(os.environ["COGLET_R3_RUNTIME_DIR"])
    out.mkdir(parents=True, exist_ok=False)
    (out / "rustc.txt").write_text(subprocess.check_output(["rustc", "-vV"], cwd=ROOT, text=True))
    shutil.copy2(ROOT / "Cargo.lock", out / "Cargo.lock")
    run_and_seal_runtime(out)


def diagnostics_dir(out):
    """Sibling of the runtime evidence dir; never part of the sealed inventory."""
    return out.with_name(out.name + "-diagnostics")


def _sync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _create_new(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(fd, "wb") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    _sync_dir(path.parent)


def write_failure_diagnostic(out, exc):
    """Best effort, never overwrites. Caller re-raises the ORIGINAL exception."""
    d = diagnostics_dir(out)
    d.mkdir(parents=True, exist_ok=True)
    text = "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))
    doc = {"schema": "coglet-r3-runner-failure/1", "type": type(exc).__name__, "message": str(exc), "repr": repr(exc), "traceback": text}
    for n in range(100):
        name = "runner-exception.json" if n == 0 else f"runner-exception-{n}.json"
        try:
            _create_new(d / name, json.dumps(doc, indent=2).encode())
            return d / name
        except FileExistsError:
            continue
    raise FileExistsError("no free diagnostic name")


def run_and_seal_runtime(out):
    try:
        run_inventory("r3_runtime", out / "runtime.log", RUNTIME_TESTS)
    except BaseException as exc:
        try:
            write_failure_diagnostic(out, exc)
        except BaseException as secondary:
            print(f"secondary failure writing R3 diagnostic: {secondary!r}", file=sys.stderr)
        raise  # the original exception; never seal after failure
    seal(out, RUNTIME_FILES)


if __name__ == "__main__":
    if sys.argv[1:] == ["--publication"]:
        publication()
    elif sys.argv[1:] == ["--runtime"]:
        runtime()
    elif len(sys.argv) == 3 and sys.argv[1] == "--aggregate":
        verify_runtime_pair(Path(sys.argv[2]), *identity())
    else:
        raise SystemExit("use --publication, --runtime, or --aggregate SAME_RUN_PAIR_DIR")
