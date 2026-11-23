# Runtime core (implementation in progress)

This crate is **not yet an M1-qualified runtime**. The current library provides:

- arrival-ordered bounded batching (`Router`);
- validated snapshot overlays with guest mount restrictions;
- a transactional host for mock `emit`, `timer`, `llm`, and `log` imports;
- independent host return calculation, lifetime effect IDs, atomic guest rollback;
- canonical ABI step records and input-driven mock replay (never output injection);
- an exclusive, fsynced, checksummed WAL with partial-tail recovery;
- a durable coordinator that persists event admission and step consumption together.
- persistent pending timers, due-wake queueing and causal one-shot consumption;
- pending actions and terminal receipt boundary validation, preserving lifetime IDs;
- deterministic heard-state validation, monotonic alignment and reserved path protection;
- trailing boundary seals (including zero-step exports) and offline boundary replay.

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
python3 crates/coglet-runtime/tools/check-primary-abi.py
```

The launcher runs the unmodified `conformance/abi/cases.json` corpus through the
official black-box harness and actual `cogletd --mock-stdio` executable: all 21
primary cases pass, with no skipped cases. It checks the fresh JUnit case names
and counts; this is a **subset gate, not full ABI conformance**. The executable
does not read case data or expected responses; it computes records through the
runtime and mock engine. Each step emits the full result and then a hash-only
frame because the harness's separate `journal_hash` assertion consumes a frame.

Rust integration tests also compare complete journals and hashes. The replay
test supplies the same independent mock program twice, drives
it with recorded inputs, compares every step, and checks that a changed program
fails. No recorded output files or return values are fed into guest execution.

## Remaining before this card can finish

- Full ABI profile adapters: module/manifest admission, trace/provenance validation,
  terminal-receipt and heard archive corpus import/validation (the runtime seams
  exist, but those language-neutral profiles have not been implemented).
- Complete pending-action/outbox lifecycle, including surface-specific source binding,
  crash-durable byte-identical mapped dispatch/reconnect reconciliation.
- The common surface state machine, catalog/grant enforcement, clock mapping,
  health/kill/disconnect, private authenticated Unix socket, and embedded C ABI.
- Native pointer/buffer/queue probes and both transport harness runs.
- `cogletd` production server mode (only `--version` and the explicit data-only
  `--mock-stdio` adapter exist; unsupported invocation fails closed).
- Engine admission/attempt-error contract and full recovery-invariant validation.
  Recovery checks all code files, pending-ledger membership, inbox wake indices,
  trace identity and journal head/chain, but not a full reconstruction of all host
  metadata from prior commits yet. Router metadata also needs semantic validation.
- Trace header/blob import/export and host-state transfer to a new segment after
  sealing. `seal_boundary` forbids later steps in that segment. The coordinator
  atomically persists the seal, inbox append/pending removal and newly queued
  inbox wakes; heard-only seals queue no wake. Surface delivery acknowledgement
  dedup/reconciliation is not implemented by this seam.
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

`Runtime::from_fixture` is test setup, not a production archive admission API.
The primary `abi.step` setup has no `initial_actions` field and therefore defaults
it to empty; future trace import must require the complete header explicitly.
The shared source sequence persists in service state and advances for committed
runtime-source events and receipt appends. Fresh fixture setup starts at zero;
production restart/segment transfer must retain the full lifetime source metadata.
