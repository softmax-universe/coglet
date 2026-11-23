#!/usr/bin/env python3
"""Regenerate language-neutral engine adapter cases using WABT wat2wasm."""
import json
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]


def module(body="i32.const 0", alloc="i32.const 0", extra="", memory="1 2"):
    return f'''(module {extra} (memory (export "memory") {memory})
    (func (export "abi_alloc") (param i32) (result i32) {alloc})
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {body}))'''


GUESTS = {
    "success": module(),
    "fresh-global": module("global.get $g global.get $g i32.const 1 i32.add global.set $g", extra="(global $g (mut i32) (i32.const 0))"),
    "runaway": module("(loop $l br $l) i32.const 0"),
    "allocation-runaway": module(alloc="(loop $l br $l) i32.const 0"),
    "invalid-allocation": module(alloc="i32.const -1"),
    "memory-too-large": module(memory="1 3"),
    "forbidden-random": module(extra='(import "wasi_snapshot_preview1" "random_get" (func (param i32 i32) (result i32)))'),
    "wrong-import-signature": module(extra='(import "coglet_v0" "timer" (func (param i32) (result i32)))'),
    "start-forbidden": module(extra='(func $start) (start $start)'),
    "memory-growth": module("i32.const 1 memory.grow i32.const 1 i32.ne if (result i32) i32.const 1 else i32.const 1 memory.grow i32.const -1 i32.ne end"),
}


def compile_guests():
    result = {}
    with tempfile.TemporaryDirectory(prefix="coglet-engine-cases-") as temporary:
        for name, wat in GUESTS.items():
            source = pathlib.Path(temporary) / (name + ".wat")
            output = source.with_suffix(".wasm")
            source.write_text(wat)
            subprocess.run(["wat2wasm", str(source), "-o", str(output)], check=True)
            result[name] = output.read_bytes().hex()
    return result


def build_cases(guests):
    def request(fixture="success", **overrides):
        value = dict(op="step", fixture=fixture, module_hex=guests[fixture], compute_mode="qualified", compute=1000,
                     memory_pages=2, host_bytes=4096, runtime_bytes=8*1024*1024, stack_bytes=65536,
                     wall_time_us=1_000_000, events_hex="81a0", step=0, logical_time=0, seed=0)
        value.update(overrides)
        return value

    def pair(req, error=None, **extra):
        return [{"send": req}, {"match": dict(ok=error is None, error=error, **extra), "within_ms": 3000}]

    cases = []
    def case(name, steps):
        cases.append(dict(name=name, timeout_ms=10000, steps=steps))
    case("fresh-instance-per-step", pair(request("fresh-global")) + pair(request("fresh-global", step=1)))
    case("runaway-deadline-and-recovery", pair(request("runaway", compute=2147483647, wall_time_us=20000), "deadline_exceeded") + pair(request(step=1)))
    case("deterministic-compute-exhaustion", pair(request("runaway"), "compute_exhausted", compute_used=1000) + pair(request("runaway", step=1), "compute_exhausted", compute_used=1000))
    case("allocation-shares-budget", pair(request("allocation-runaway"), "compute_exhausted", compute_used=1000))
    case("linear-memory-growth-cap", pair(request("memory-growth")))
    for fixture, error in [("invalid-allocation", "invalid_pointer"), ("memory-too-large", "memory_exhausted"),
                           ("forbidden-random", "unqualified_module"), ("wrong-import-signature", "unqualified_module"),
                           ("start-forbidden", "unqualified_module")]:
        case(fixture, pair(request(fixture), error))
    case("zero-compute-is-not-unlimited", pair(request(compute=0), "invalid_argument"))
    case("signed-ffi-overflow-refused", pair(request(compute=2147483648), "invalid_argument"))
    case("unqualified-aot-refused", pair(request(compute_mode="aot"), "unsupported_mode"))
    case("unknown-compute-mode-refused", pair(request(compute_mode="unknown"), "unsupported_mode"))
    return cases


if __name__ == "__main__":
    cases = build_cases(compile_guests())
    (ROOT / "conformance/engine/cases.json").write_text(json.dumps(cases, indent=2) + "\n")
    print(f"generated {len(cases)} engine cases")
