# WAMR engine

`Wamr::interpreter()` implements `coglet_engine::Engine`. `load` validates bytes
and limits without executing guests. `instantiate` creates a one-shot descriptor;
`Instance::handle` instantiates a **fresh native instance**, runs initialization,
allocation and handle under ONE allowance/deadline, then discards all native state.
There is no persistent guest instance to accidentally reuse after a failed call.

## Build and accounting policy

Builds require CMake, a C/ASM compiler, curl and tar. `build.rs` fetches WAMR commit
`0e47872fb4e57833dbeccf7c312a406fa2dac2bc` (2.4.3), checks archive SHA-256
`3d051b3c332813aa217f59ac6b3e46bba1de892c915c4a6b54db9983640bbca7`, and builds it
with the committed `native/CMakeLists.txt`. No environment-provided libiwasm is
accepted. The authenticated archive is cached in Cargo's OUT_DIR. Source is
re-extracted before each rebuild so stale local patches cannot accumulate.

Compute mode: **`wamr-2.4.3-fast-cumulative-v1`**.
Build ID: **`0e47872fb4e57833dbeccf7c312a406fa2dac2bc+cumulative-v1`**.
This pinned policy contains TWO explicit source changes:

1. Fast-interpreter accounting decrements `exec_env->instructions_to_execute`
   directly, rejects nonpositive remaining counts, and retains the exact remaining
   count across exported calls AND nested guest function calls. Upstream's setter
   alone only initializes a call-local counter and cannot prove this contract.
2. Native instantiation executes **no guest hooks**. Reactor `_initialize` (or
   standalone `__wasm_call_ctors` if no reactor initializer exists) runs explicitly
   after installing the meter/watchdog. Legacy `__post_instantiate` is unqualified
   and rejected; Wasm start sections are forbidden by the ABI.

These are build-specific fast-interpreter units, NOT portable Wasm opcode fuel.
FFI budgets are strictly positive signed i32 and never truncated. Exhaustion
prevents another guest call. The shared native runtime/allocator/registry lifetime
is serialized process-wide; waiting for it never renews the absolute deadline.
The watchdog is cancellable and joined BEFORE native instance destruction on all
paths, including unwinding. Native host callbacks are trusted, cooperative,
bounded/cancellable and must never wait for network or reenter this engine.
A guest termination flag cannot preempt arbitrary blocking trusted host code.

The default feature selects the interpreter. `aot` selects an unsupported default
mode; it does not compile/execute unqualified AOT, pretend wall time is compute,
or permit shared memory. `Wamr::aot()` fails admission even on malformed bytes.
`Wamr::interpreter()` remains explicit when both features are built.

## Memory/stack boundaries

- Initial AND declared maximum linear pages are checked before native execution;
  memory is unshared wasm32 with exactly one exported `memory`. Growth is also
  capped by WAMR's page limit. Zero, imported/unbounded memory and over-grants fail.
- `runtime_bytes` bounds WAMR's **non-linear allocator pool** (including module
  metadata and execution-stack allocations). Linux linear memory uses mmap, not
  this pool, and is bounded separately by the page grant. The regression test
  exhausts metadata with many tiny functions, not a misleading linear-page test.
- `stack_bytes` bounds the interpreter execution stack. Native watchdog stack is
  fixed at 128 KiB. Rust parser/module/event buffers and caller-owned host state
  are separate; this is **not a total process RSS/OS stack limit**. A runtime that
  requires a total RSS/process cap must provide qualified process isolation or
  refuse that route. Never advertise an EO1 128 MiB native RSS result from QEMU.
- Aggregate `host_bytes` charges event copying and every successful import-memory
  read/write. Pointers use widened range checks; copies cannot expose native
  addresses or outlive a callback. Memory faults are sticky even if a host callback
  accidentally ignores a returned error.

## Admission and integration

Production validation pins R1/R2/R5 features and import signatures. No imported
memory, threads, SIMD, memory64, components or start section. The four ABI imports
and the restricted WASI table are wired only to the supplied `Host`; libc/WASI
ambient filesystem, clocks, randomness and environment are compiled OUT.
`proc_exit`, including zero, always aborts rather than delegating to host policy.

For R3 the validator requires the exact canonical-NaN instruction sequence after
every specified f32/f64 producer, not an untrusted custom-section assertion or an
identity transform. Positive/negative tests cover all listed producers; the SAME
NaN tests execute on x86_64 and ARMv7 QEMU. This validates incoming rewritten code;
it does not implement the hub's separate publish-time transform/provenance gate.

`Host` must use pinned recorded admission state and transactional overlays, enforce
filesystem mount/descriptor/capability rules, recompute results on replay, and
**discard every staged effect on any failed StepOutcome**. The engine never
injects recorded host returns or directly commits/dispatches effects. Runtime
journal durability and filesystem semantics remain with their respective crates.

## Verification

```sh
cargo test --locked -p coglet-engine -p coglet-engine-wamr
cargo test --locked -p coglet-engine-wamr --no-default-features --features aot
cargo build --locked -p coglet-engine-wamr --example wamr-conformance
conformance/run --junit target/engine-registered.xml
rustup target add armv7-unknown-linux-gnueabihf
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc \
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER='qemu-arm -L /usr/arm-linux-gnueabihf' \
cargo test --locked -p coglet-engine-wamr --target armv7-unknown-linux-gnueabihf -- --test-threads=1
```

The existing platforms spike's `armv7-qemu` CI job runs these real crate tests in
addition to its historical spike. Supported build targets are Linux x86_64 and
ARMv7 GNU hard-float/QEMU; other targets fail closed at build time. iOS and actual
EO1/sysroot/RSS acceptance are not claimed by this crate's current qualification.
