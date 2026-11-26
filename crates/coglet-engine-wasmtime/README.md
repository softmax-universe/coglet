# Wasmtime verification backend

`Wasmtime` implements `coglet_engine::Engine` with pinned Wasmtime **51.0.0**,
Cranelift, and compute mode `wasmtime-51.0.0-cranelift-fuel-v1`. It does not use
ambient WASI, deserialize native modules, or enable engine-side NaN scrubbing.
ABI v0 permits another qualified metering engine; its interpreter-only WAMR
restriction does not require Wasmtime to interpret. No spec change is made here.

## Boundaries

- Shared `coglet_engine::admission` is the unchanged production WAMR validator:
  exact import signatures, pinned features, no start, declared memory maximum,
  and structural validation of every R3 suffix. Loading never executes guest code.
- One store fuel allowance spans initialization, allocation, host work and handle.
  Host explicit compute charges reduce that same remaining fuel. Fuel units and
  exhaustion safe points are **not comparable to WAMR**; no cross-engine cutoff
  equality is promised. Unknown/AOT modes fail before instantiation.
- One absolute deadline covers the attempt. A cancellable, joined watchdog raises
  the epoch on that instance's private engine. Concurrent stores cannot interrupt
  each other. Compilation is synchronous, not preemptible; the deadline is checked
  again before guest execution. This is guest interruption, not a compiler-time
  or trusted-native-callback preemption guarantee.
- `memory_pages` caps admitted linear-memory initial and declared maximum.
  `runtime_bytes` additionally bounds aggregate store linear/table growth (16
  conservative accounted bytes per table element). `stack_bytes` sets Wasmtime's
  maximum guest stack. Module input is bounded by the shared admission rule.
  **These do not cap process RSS, compilation allocations, executable code or
  Wasmtime's virtual-address reservations**. Failed growth above a declared
  maximum returns -1 without charging; permitted allocations which subsequently
  fail roll back their provisional charge and abort with MemoryExhausted (never
  reuse an ambiguous failed-allocation store). Unlike WAMR's allocator-pool field,
  `runtime_bytes` is an accounted store-resource bound, not an allocator arena.
- Only copying `GuestMemory` access reaches the supplied pinned-state host. Byte
  accounting is cumulative, errors cannot be swallowed by a callback, proc_exit
  always aborts, and no recorded return is injected during replay. The host must
  remain bounded/cooperative. Caller discards staged effects on any failure.
- Wasmtime requires `'static` store data. A private, non-Send store holds an erased
  pointer to a stack-scoped host-reference slot; synchronous callbacks alone
  borrow it. Store/linker/callbacks never escape `handle`, and the watchdog owns
  only the engine. This lifetime boundary is the backend's only unsafe operation.

## Verification

```sh
cargo test --locked -p coglet-engine-wasmtime -p coglet-engine-wamr
cargo clippy --locked -p coglet-engine-wasmtime -p coglet-engine-wamr --all-targets -- -D warnings
cargo build --locked -p coglet-engine-wasmtime --example wasmtime-conformance
conformance/run --suite engine --impl 'python3 conformance/engine/adapter.py target/debug/examples/wasmtime-conformance'
```

The registered `wasmtime-fuel` runs the **same** 14 black-box engine cases as
WAMR, without fixture-name dispatch. Workspace CI also executes the named
`engine_trait_conformance` and `cross_engine_state_hash` integration tests.

The differential test executes identical core-Wasm bytes and inputs in both
engines, with fresh instances each step and prior raw state as the next input.
It compares raw bytes and the actual `coglet_fs::Snapshot::tree_hash()` (Git
SHA-256 tree id, no float normalization), including code and state files. Four
guest programs cover evolving integers, every R3 scalar f32/f64 operation,
promote/demote, and bit-exact sign/payload operations; adversarial inputs include
zeros, subnormals, infinities and NaN payloads. Unrewritten arithmetic is refused
by both engines. The capture host is **test-only**, not a production log decoder
or a runtime transactional filesystem implementation.

This is native-host verification, not cross-architecture/AOT qualification,
publication-transform fuzz qualification, real runtime journal/replay admission,
or native EO1/iOS support. Successful state equality does not imply equal
resource-exhaustion boundaries or equal engine metadata/journal hashes.
