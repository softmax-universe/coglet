# ADR 0002: WAMR platform findings (ARMv7 and iOS)

- Status: **proposed**
- Date: 2026-11-23
- Scope: platform feasibility spike, not a product ABI change or engine selection freeze

## Context

The [runtime platform notes](../design/coglet-runtime.md#platform-notes) require
validation before depending on WAMR for the EO1; iOS must use interpreter-only
execution. This spike is independent of the skeleton/spec work and lives in
[`spikes/wamr-platforms`](../../spikes/wamr-platforms/README.md). No source from
retired implementations is ported and no normative `spec/` file is changed.

Read-only hardware context: Baby Bear `pods/eo1/README.md`, `AGENTS.md`,
`docs/DEVELOPMENT.md` and `tools/build_health.sh`. These identify the i.MX6 stack,
ARMv7 builds and inherited Ubuntu 18.04 baseline and explicitly distinguish
emulator checks from hardware acceptance. No live device operation was attempted.

## Experiment

- WAMR 2.4.4, pinned commit `8c18e3f68b16c4bcaf05996b2636f6ed2b4cf629`.
- Rust 1.91.1, `armv7-unknown-linux-gnueabihf`; GCC cross compiler 13.3.0;
  Ubuntu 24.04 cross sysroot (glibc 2.39); QEMU 8.2 user mode, Cortex-A9.
- WAMR classic interpreter, instruction metering enabled; AOT/JIT/Fast JIT,
  Memory64, WASI, SIMD and guest threads disabled. Software bounds checks.
- WAMR C built MinSizeRel; Rust size-optimized release, LTO, stripped.
- One small core-Wasm module, memory initial 1/max 2 pages, 16 KiB interpreter
  stack, zero application heap; fixed 2 MiB WAMR allocator pool.

Run `bash spikes/wamr-platforms/test-armv7.sh`. The dedicated
`.github/workflows/wamr-platforms.yml` runs this command and uploads fresh raw
results. Checked-in [`evidence/`](../../spikes/wamr-platforms/evidence/) is a
local 2026-11-23 run; it is not presented as a hosted CI run.

## Observed ARMv7 results

All assertions passed, exit 0:

- Host pointer width **32 bits**; exported `answer()` returns **42**.
- Guest i64 `0x12345678ffffffff + 1` returns `0x1234567900000000` through two
  32-bit API cells (including carry), not a truncated host pointer/usize.
- Growing initial 1 page by 2 fails at declared max 2 pages (`-1`).
- Store/load at byte 65,532 succeeds; load at 65,536 traps out of bounds.
- Infinite loop traps with `Exception: instruction limit exceeded` at configured
  cap **10,000**; clearing exception and setting a fresh budget allows another
  call returning **42**. Test timeout is only an outer safety net.

| Measurement | Local result | Boundary |
|---|---:|---|
| Stripped ELF file | 400,472 bytes | Rust host + linked WAMR, dynamically linked system libraries excluded |
| ELF text / data / BSS | 388,282 / 9,484 / 2,097,360 bytes | BSS includes the fixed 2 MiB pool, not file size |
| WAMR usable pool | 2,096,936 bytes | Pool management overhead removed |
| WAMR current / high-water allocation | 19,352 / 19,352 bytes | Module + instance + execution environment; excludes linear-memory mmap |
| Guest linear memory | 65,536 bytes | One initial Wasm page; max 131,072 bytes; boundary word touched |
| Maximum host RSS | 12,564 KiB | GNU time on QEMU/timeout; **not native ARM/EO1 RSS** |

These are measurements for this tiny guest, not many-coglet forecasts. The pool
reservation is not the whole runtime's memory limit: mmap-backed linear memory,
Rust allocations, native stack and libraries lie outside it. Product admission
must separately enforce linear-memory and aggregate process budgets. A physical
EO1 run must measure native RSS/peak under realistic workloads.

**Compatibility limitation:** ELF symbol versions include **GLIBC_2.34** (see
`evidence/elf-versions.txt`). This modern-sysroot binary is not suitable for an
unmodified Ubuntu 18.04 frame. A device-compatible sysroot or separately tested
static artifact is still needed. QEMU proves ARMv7 code execution, not the EO1
kernel/userspace/hardware. The design's physical-EO1 verification gate remains.

## iOS outcome

Linux `build-ios.sh` was executed and exits **2**, explicitly reporting absent
`xcrun` / iPhoneOS SDK. This host has no Apple SDK or Apple linker; no iOS archive
or executable was built. Rust target installation alone cannot supply the Darwin
platform headers, system stubs and linker. A licensed SDK plus compatible Linux
cross-toolchain might work, but was not supplied or tested; do not claim iOS is
impossible from Linux in principle.

The [README](../../spikes/wamr-platforms/README.md#ios-stop-at-the-sdk-boundary)
provides exact Mac prerequisites and commands. `build-ios.sh` configures CMake
for `iOS`, `arm64`, `iphoneos`, deployment target 15.0, builds a static WAMR
archive with the **same interpreter-only** feature settings and then links this
Rust host for `aarch64-apple-ios`. These Mac steps are unexecuted. Signing/app
packaging/device testing are a later separate acceptance boundary; there is no
simulator or device result. No JIT or AOT capability is needed by this probe.

## ABI implications / proposed constraints

These are inputs to the ABI owners, not unilateral spec amendments:

1. **Guest addresses are wasm32 offsets**, never host pointers. The host is
   32-bit on EO1 and 64-bit on iOS. Use explicit u32 lengths/offsets and checked
   `offset + length` arithmetic (widen before adding, validate against current
   memory, reacquire native addresses after growth). Do not serialize `usize`,
   C `long`, or pointer-width layouts. Host pointers stay opaque within the host.
2. **i64 is usable on ARMv7.** Keep logical time, step IDs and seeds at explicit
   64-bit widths when needed; no 64-bit CPU is required for Wasm i64 arithmetic.
   WAMR's cell API uses two u32 cells, low then high, for i64. Encode/decode bytes
   explicitly; avoid misaligned casts to native u64 pointers. This experiment
   tests call/result arithmetic, not host callbacks, atomics or every i64 opcode.
3. **No Memory64 dependency.** Memory64 is disabled. Wasm32's nominal 4 GiB space
   is not a viable allocation promise on 32-bit hosts; address space, WAMR limits
   and process overhead reduce it. Require declared per-module maxima and modest
   configurable admission limits; this probe tests only a 128 KiB declared max.
4. **Compute cap API is signed C int (32-bit here).** Reject invalid, zero,
   negative and out-of-range host budgets before FFI; negative means unbounded
   upstream. Reset the budget per call and distinguish compute exhaustion from
   ordinary traps. This spike hardcodes a valid 10,000 budget. Instruction units
   are WAMR-specific, not portable fuel, and do not bound blocking host functions.
   Wall-clock interruption and metering equivalence belong to the parallel spike.
5. **No architecture-dependent wire structs.** Align/pack serialized values by
   the ABI specification, not Rust/C native layout. Use deterministic bytes and
   explicit integer widths. Platform width changes may change memory footprint,
   not the coglet's semantic state. Floating-point determinism is not tested here.

## Recommendation and remaining gates

Retain WAMR behind the planned Engine interface and a wasm32/no-Memory64 ABI;
continue interpreter-only platform work. This is **partial platform validation**,
not approval to ship to EO1/iOS. Before product rollout: use an EO1-compatible
sysroot and run on identified hardware; run the documented iOS archive/link probe
on Xcode and a signed device harness; then exercise the actual WASI filesystem,
callbacks and multi-coglet aggregate budgets through the shared conformance suite.
The ADR remains proposed pending independent review and the spec freeze.
