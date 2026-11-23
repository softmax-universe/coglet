# ADR-0003: Determinism rules for coglet Wasm modules

- Status: Proposed
- Date: 2026-11-23
- Authors: default (Hermes worker), task t_01a205d4af9cc355
- Related PR: https://github.com/softmax-universe/coglet/pull/5
- Supersedes / superseded by: none
- Evidence: [`spikes/cross-engine-hash/`](../../spikes/cross-engine-hash/) and its CI job `spike-cross-engine-hash`
- Relates to: `docs/design/coglet-runtime.md` §Revised runtime model; plan ground rule 5 ("both engines must reach identical state hashes")

## Context

Coglets are plain core Wasm. The runtime may run the same coglet on WAMR (interpreter or AOT) or on
Wasmtime, and replay, git sync and conformance all rest on state hashes being identical for the same
inputs. The Wasm spec leaves some behaviour to the implementation. We built a 1,000-step guest
(f32/f64 math, sqrt, NaN-producing operations, state blob hashed as raw bits) and ran it on
Wasmtime 51, WAMR 2.4.6 fast interpreter and WAMR 2.4.6 AOT (`wamrc`) on x86-64.

## What we measured

| Experiment | Result |
|---|---|
| Guest as compiled by rustc, Wasmtime vs WAMR interpreter | Equal for 1000/1000 steps |
| Same, WAMR AOT vs the others | **Diverges at step 11** (then almost every step). A NaN lane differs only in payload bits (`0x7ff82b108398f114` vs `0x7ffb08801868363a`). Likely cause (not bisected): AOT goes through LLVM, which may reorder operands of commutative ops, and x86 keeps the payload of one particular operand. |
| Wasmtime with `cranelift_nan_canonicalization(true)` vs WAMR | **Diverges at step 0.** WAMR has no canonicalization option, so turning it on in Wasmtime alone moves Wasmtime away from WAMR. |
| Guest with the R3 transform below (see Decision), all four of: Wasmtime, Wasmtime+canonicalization, WAMR interpreter, WAMR AOT | **Equal for 1000/1000 steps**, host-side and guest-side hashes |
| Guest importing `wasi clock_time_get` or `random_get` | Same engine, same inputs, two runs: different hashes from step 0 |
| Guest using relaxed-simd (`f32x4.relaxed_madd`, `i32x4.relaxed_trunc_f32x4_s`) | Wasmtime runs it. WAMR's interpreter refuses to load it (`invalid opcode 0xfd 105`) and `wamrc` cannot compile it. Not portable even before asking whether results match. |
| Guest using atomics (single-threaded) | Identical on all configs, because nothing races. Atomics need a WAMR build flag and a Wasmtime feature to be accepted at all. Thread interleaving was **not** measured. |

Not a source of difference here: integer and float arithmetic other than NaN payloads, `sqrt`
(a single correctly rounded instruction everywhere), float-to-int traps, and `sin/cos/exp/ln/pow`
when they come from libm code compiled into the guest rather than from the host.

## Decision (proposed)

Five rules. R1, R2, R4 and R5 are checked by `xhash-wasmtime lint` over the module bytes. R3 is a
module transform. All are applied by the hub at build-then-commit; the hash of record is for the
transformed module.

**R1. No threads.** Reject atomic operators, shared memories and any thread-spawn import.
Engines run with threads off (Wasmtime `wasm_threads(false)`; WAMR built without shared memory,
`LIB_PTHREAD` and `LIB_WASI_THREADS`).

**R2. No relaxed-simd.** Reject any relaxed operator. Engines run with it off (Wasmtime
`wasm_relaxed_simd(false)`). Fixed-width simd128 is also excluded for now (R5): its NaN results need
the same treatment as scalar floats and WAMR builds it only behind a flag.

**R3. NaN results are canonical, by module transform and not by engine flag.** After every float
operator whose NaN payload the spec leaves open (add, sub, mul, div, sqrt, ceil, floor, trunc,
nearest, min, max, promote, demote), the build rewrites the module so a NaN result becomes the
positive quiet NaN (`0x7fc00000` / `0x7ff8000000000000`). Operators the spec defines bit-exactly (neg,
abs, copysign, load, store, reinterpret) are untouched, so NaN payloads from events pass through
state unchanged on every engine. The rewrite is `local.set t; select(local.get t, NaN, f.eq t t)`,
plain core Wasm, so it holds on interpreters and AOT alike. Cost on the spike guest: 421 operators
wrapped, wasm 60 KB to 66 KB, AOT 103 KB to 119 KB, no visible slowdown at this size.
Wasmtime's `cranelift_nan_canonicalization` is optional once R3 is applied (equal with it on or off)
and must not be relied on alone.

**R4. Time, randomness and I/O ordering come only from inputs.** The module may import only an
allowlist: WASI `args_*`, `environ_*`, `proc_exit`, and file operations on `/code`, `/data`, `/logs`
(`fd_*`, `path_*`). Everything else is rejected, in particular `clock_time_get`, `clock_res_get`,
`random_get`, `poll_oneoff`, `sched_yield`. Logical time and the seed arrive as `handle` arguments.
Any future coglet host function must be a pure function of its arguments and of the files in the
coglet's own tree. The runtime must not expose wall clock, randomness, directory iteration order that
isn't sorted, or environment-dependent values.

**R5. Pinned feature set.** The module must validate with exactly: mutable globals,
sign-extension, saturating float-to-int, multi-value, bulk memory, reference types. Both engines
are configured to the same set. Anything newer is added by a later ADR with a cross-engine test.

## Verification

`scripts/run-matrix.sh` (CI) asserts: the conforming guest passes lint and has identical per-step
hashes on all four configurations; each counterexample is rejected by lint with the expected rule
(R2 relaxed-simd, R4 clock, R4 random, R1 atomics, plus R3 by removing the transform); and the
divergence or load failure shows up when the rule is not applied. The per-step logs are uploaded as
a CI artifact.

## Consequences

- Authors may use floats freely; the SDK need not scrub NaNs by hand. Modules are rewritten at publish.
- A bug in the transform is a determinism bug, so it needs its own fuzz and differential tests
  before this goes to `accepted`.
- Canonicalizing after each op hides the engine's NaN differences, but also means coglets cannot
  carry information in NaN payloads they compute. Payloads they merely copy are preserved.
- Rules apply to the guest. They do not make the runtime's own scheduling (batching, timers)
  deterministic; that is covered by journaling event batches.

## Open questions (not measured)

1. **aarch64 and 32-bit ARM.** Only x86-64 was run. NaN payloads and fused multiply-add contraction in AOT
   differ by architecture. WAMR AOT on ARM needs a cross-architecture CI leg (wamrc there needs LLVM).
2. **Resource limits.** `memory.grow` failure, stack exhaustion depth and "compute cap" trap points
   differ per engine unless limits are defined in terms that every engine counts the same way. Not
   tested. Likely need R6: memory ceiling declared in the manifest and enforced identically, and a cap
   expressed as host-metered inputs rather than engine fuel.
3. **Threads.** The argument for R1 is spec-level (interleavings are not defined). A real
   wasi-threads counterexample would turn it into a measurement.
4. **WAMR variants.** Classic interpreter, `wamrc --opt-level`, `--enable-fp-contract`-like options, and the JIT tiers
   were not varied. Any that change float codegen need to be pinned in the engine config.
5. **Spec versions.** Wasm 3.0 additions (GC, exceptions, tail calls, memory64) are excluded by R5 until tested.

## Alternatives

- **Engine flags only** (`cranelift_nan_canonicalization`): measured, diverges from WAMR at step 0 because WAMR has no equivalent. Rejected as the sole mechanism.
- **Author discipline** (SDK scrubs NaNs before storing): unenforceable for arbitrary guest languages, and misses NaNs that are only hashed or compared bitwise. Rejected.
- **Hash a canonical view of state** (canonicalize NaNs when hashing, not when computing): hides the divergence in the hash but lets coglet logic branch on payload bits (`to_bits`) differently per engine, so replay can still fork. Rejected, but cheap to add as a belt-and-braces measure.
- **One engine only**: contradicts plan rule 5 and the iOS/EO1 constraints.

## Approval

Not yet reviewed. Pending independent review of PR #5; this ADR is `Proposed` and is not a decision until accepted.
