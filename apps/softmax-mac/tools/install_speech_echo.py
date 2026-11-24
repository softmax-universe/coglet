#!/usr/bin/env python3
"""Compile and install the model-free speech guest and trusted embedding JSON.

No fixture host actions, guest_script or canned event responses are installed.
The output is operator-owned configuration, not a surface wire message or signed
hub publication. Run from a source checkout with Rust wasm32-wasip1 installed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "conformance"))
from wire import encode  # release-pinned deterministic CBOR profile

COGLET = "coglet:" + "a" * 64
SURFACE = "surface:" + "b" * 64
COMPUTE_MODE = "wamr-2.4.3-fast-cumulative-v1"


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def run(command, **kwargs):
    subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def install(output, wal, trap=False):
    output = Path(output).resolve()
    wal = Path(wal)
    if not wal.is_absolute():
        raise ValueError("--wal must be an absolute installed writable path")
    # A new trace seed must not be silently installed over a live runtime's WAL.
    if wal.exists() or (output / "runtime.json").exists():
        raise ValueError("refusing to replace existing runtime configuration or WAL")
    output.mkdir(parents=True, exist_ok=True)
    output.chmod(0o700)
    target = ROOT / "target" / ("speech-echo-trap" if trap else "speech-echo")
    manifest = ROOT / "coglets/speech-echo/Cargo.toml"
    command = ["cargo", "rustc", "--manifest-path", str(manifest), "--target", "wasm32-wasip1",
               "--release", "--lib", "--locked"]
    if trap:
        command += ["--features", "trap-injection"]
    command += ["--", "-C", "link-arg=--max-memory=16777216"]
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    run(command, env=env)
    raw = (target / "wasm32-wasip1/release/coglet_speech_echo.wasm").read_bytes()
    guest = output / "step.wasm"
    guest.write_bytes(raw)
    run(["cargo", "run", "--quiet", "--locked", "-p", "coglet-runtime", "--features", "native-engine", "--example", "qualify_apple_guest", "--", str(guest)])
    wasm = guest.read_bytes()
    if not wasm.startswith(b"\0asm\x01\0\0\0"):
        raise ValueError("compiler output is not core Wasm")
    source_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    recipe = json.dumps(command, separators=(",", ":")).encode()
    provenance = {
        "version": "0.1", "source_commit": source_commit,
        "original_sha256": digest(raw), "rewritten_sha256": digest(wasm),
        "recipe_sha256": digest(recipe),
        "policy_sha256": digest((ROOT / "docs/adr/0003-determinism-rules.md").read_bytes()),
        "transform_sha256": digest((ROOT / "spikes/cross-engine-hash/host-wasmtime/src/nancanon.rs").read_bytes()),
    }
    budgets = {"compute": 100_000_000, "compute_mode": COMPUTE_MODE, "memory_pages": 256,
               "wall_time_us": 5_000_000, "max_events": 16, "host_bytes": 1_000_000}
    manifest_value = {"version": "0.1", "id": COGLET, "subscriptions": ["speech.end"],
                      "required_capabilities": ["speech.speak_chunk"],
                      "exclusive_outputs": ["audio.mic", "audio.speaker"], "budgets": budgets}
    setup = {"manifest": manifest_value,
             "initial_files": {"/code/bin/step.wasm": wasm.hex(),
                               "/code/build/provenance.cbor": encode(provenance).hex()},
             "initial_effect_ids": {"actions": [], "timers": [], "llm_requests": []},
             "initial_actions": [], "initial_timers": [], "initial_llm_requests": [], "initial_inbox_wakes": []}
    runtime = {"wal": str(wal), "setup": setup,
               "native_schedule": {"trace_id": uuid.uuid4().hex, "seed": int.from_bytes(os.urandom(8), "big"),
                                   "runtime_bytes": 33_554_432, "stack_bytes": 262_144}}
    binding = {"surface": SURFACE, "permitted_coglets": [COGLET]}
    for name, value in [("binding.json", binding), ("runtime.json", runtime), ("build.json", provenance)]:
        path = output / name
        path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        path.chmod(0o600)
    print(json.dumps({"binding": str(output / "binding.json"), "runtime": str(output / "runtime.json"),
                      "wasm": str(guest), "trap_injection": trap}))
    return binding, runtime


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--wal", required=True, type=Path)
    parser.add_argument("--trap-injection", action="store_true", help="TEST ONLY: compile guest unreachable on speech.end")
    args = parser.parse_args()
    install(args.output_dir, args.wal, args.trap_injection)


if __name__ == "__main__":
    main()
