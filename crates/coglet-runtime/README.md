# Runtime core (implementation in progress)

This crate is **not yet an M1-qualified runtime**. The current library provides:

- arrival-ordered bounded batching (`Router`);
- validated snapshot overlays with guest mount restrictions;
- a transactional host for mock `emit`, `timer`, `llm`, and `log` imports;
- independent host return calculation, lifetime effect IDs, atomic guest rollback;
- canonical ABI step records and input-driven mock replay (never output injection);
- an exclusive, fsynced, checksummed WAL with partial-tail recovery;
- a durable coordinator that persists event admission and step consumption together.

`Runtime::from_fixture` and `MockEngine` implement the data-only engine seam in
`conformance/abi/README.md`. Scripted engine usage is not a real Wasm measurement.
The crate currently owns its `Engine` and snapshot seams because the shared
`coglet-engine` and `coglet-fs` APIs were empty at implementation start. Those
seams must be integrated when their owning lanes publish the real APIs.

## Verification

```sh
cargo test -p coglet-runtime
cargo clippy -p coglet-runtime --all-targets -- -D warnings
cargo fmt -p coglet-runtime -- --check
```

Rust integration tests independently execute the main ABI case corpus and
compare complete journals and hashes. This is **not** a language-neutral harness
pass. The replay test supplies the same independent mock program twice, drives
it with recorded inputs, compares every step, and checks that a changed program
fails. No recorded output files or return values are fed into guest execution.

## Remaining before this card can finish

- Full ABI profile adapters: module/manifest admission, trace/provenance validation,
  terminal receipts, trailing boundaries, and heard-state archive semantics.
- Timer firing and persistent pending-action/outbox lifecycle, including
  crash-durable byte-identical mapped dispatch/reconnect reconciliation.
- The common surface state machine, catalog/grant enforcement, clock mapping,
  health/kill/disconnect, private authenticated Unix socket, and embedded C ABI.
- Native pointer/buffer/queue probes and both transport harness runs.
- `cogletd` packaging and command-line entrypoint (the binary remains the skeleton).
- Engine admission/attempt-error contract and full recovery-invariant validation.
- Independent exact-head review, green CI, merge, and board review handoff.

The internal WAL format is not the exported ABI journal. Each internal commit
contains runtime host state and an optional ABI record in one checked frame;
`DurableStore::export_journal` emits only u32be-framed deterministic CBOR ABI
records. Complete corrupt frames fail closed; only incomplete trailing bytes are
truncated. Silently rolling back a complete corrupt commit could re-dispatch an
already executed external effect and is not an acceptable recovery strategy.

The supervisor must supply a private persistent directory for WAL storage.
Public coordinator event admission assumes prior surface authorization; reserved
runtime/inbox events are refused at that external seam.
