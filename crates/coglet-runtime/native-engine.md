# Opt-in native scheduler (bounded qualification)

Build on the WAMR-qualified Linux targets with CMake available:

```sh
cargo build -p coglet-runtime --features native-engine --locked
cargo test -p coglet-runtime --features native-engine --locked
```

The existing `--surface-service-socket SOCKET BINDING_JSON UID WAL SETUP_JSON`
mode enables guest scheduling when SETUP_JSON contains a `native_schedule` object:

```json
{"trace_id":"11111111111111111111111111111111","seed":7,"runtime_bytes":8388608,"stack_bytes":65536}
```

For C embedding, pass the same optional `native_schedule` alongside `wal` and
`setup` to `coglet_runtime_start`. These are trusted supervisor inputs, never
surface wire messages. Builds without the feature refuse a requested scheduler;
there is no mock fallback. The current engine does not build on iOS/macOS.

Setup's existing immutable `/code/bin/step.wasm`, provenance and manifest pins
are still required. Use the actual WAMR compute mode
`wamr-2.4.3-fast-cumulative-v1`; unsupported modes and over-budget declared
maximum memory are refused before input consumption. This first route admits
only `coglet_v0` imports: `emit`, `timer`, `llm`, `log`. All WASI imports are
explicitly refused until the transactional filesystem host is qualified. LLM
calls can stage requests but this card does not supply a provider/gateway pump.

Each native pump schedules at most one arrival-ordered batch. No guest executes
without current authenticated manifest operation/subscription grants and
exclusive leases. Current grants, actual engine metadata and durable step-derived
seed are recorded, not caller-provided engine metadata. One absolute deadline
covers load, instantiation, initialization, allocation, copy-in and handle. Every
attempt gets a fresh instance; failed attempts discard staged effects and commit
one failure record before consuming the batch. Successful action arrays commit
in invocation order before the transport pump can see them. After execution,
the supervised clock is sampled again for each dispatch authorization; a lease
that expired during guest execution cannot prepare a new envelope.

Usage exposes measured compute/copy bytes and wall duration. Since the shared
engine currently does not report peak linear pages, `memory_pages` conservatively
records the admitted cap, not measured peak/RSS. Native wall duration is
nondeterministic; exact native archive replay/usage comparison remains part of
the serial ABI qualification, not a claim made by the current mock replay tool.
Scheduling requires a connected, healthy authenticated surface; disconnected
background-only execution is not implemented.

`tests/native_operational.rs` executes real Wasm through actual C pointer entry
points and a separate actual cogletd socket process. It checks call-order acts,
trap rollback, persisted usage/build identity, one-time timer consumption, and
byte-identical old-ID reconciliation after clock advance/reopen. A separate
expensive-initializer regression verifies dispatch uses a post-execution clock
when the original grant has expired. This is not full native speech/device I/O
qualification or final operational acceptance.
