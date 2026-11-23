# Factory DB guest slice — in progress

**Non-deployable ABI 0.2 slice**, not a qualified DB runtime. Existing WAMR/mock/
runtime admission is unchanged and must reject `coglet_v0.db`. No LLM, executive,
dispatcher or ambient time/randomness. No deployment manifest with a placeholder
catalog hash: approved catalog admission/migrations/recovery must come first.

## Implemented

- Portable forward schema source `migrations/0001.sql`: objects, relations,
  semantic events, work_queue, outbox and schema_migrations; foreign keys,
  deterministic-key indexes and explicit optimistic versions. JSON stays opaque
  text. No SQLite JSON functions, replace/upsert or SQLite-specific SQL types.
- Repository `get_object`, `query_objects`, `get_relations`, `create_object`,
  optimistic `update_object`, `append_event`, and dedicated `enqueue_outbox`.
  Multiple calls share the host step transaction; there is no guest commit.
  Propagate **every** error out of handle. Success is provisional, never durable
  DB commit. Outbox is never mutated through guest SQL.
- Query pages retain revision/opaque cursor, never fetch unbounded results.
  Authentication/expiry is the host's responsibility and not implemented here.
- Application ULID encoding uses supplied recorded timestamp/entropy; callers must
  preserve uniqueness across trace resets. ABI outbox action/correlation IDs retain
  the frozen 32-lowercase-hex encoding, not 26-character ULIDs.
- Read-only `factory.inspect` probe event with payload `{id: <object id>}` reads one
  object and stages an ID/status log. DB rejection propagates to nonzero handle
  with no log. This is a walking-skeleton probe, not a product executive/API.

## Verification and limits

```sh
# from repository root
bash coglets/factory/check.sh
```

Runs SDK buffer tests, native repository tests, clippy/fmt, a bounded-memory
wasm32-wasip1 reactor build, and execution of the actual four-buffer import through
a **test-only Python Wasmtime/SQLite transport**. Checks import signature, disjoint
live buffers, canonical CBOR, real query readback, rejection propagation, absence
of ambient clock/random imports, and a 16 MiB memory maximum. The existing Rust CI
workflow calls this script.

Native repository tests execute real SQLite queries and schema constraints. The
outbox forwarding test is only a typed transport unit test; it does not implement
or prove native immutable-key dedup. Neither transport is a production capability
adapter. No admission/grants, host-byte reservation, cancellation, journal/replay,
marker recovery, sticky poison or authenticated pagination qualification is claimed.
No frozen conformance input changed. These tests do **not** satisfy the task's
seven named acceptance cases through the shared real/mock runtime suite.

## Required next

1. Pin typed catalog parameters/columns/table access/unique ordering and digest in
   the host; catalog.rs is deployment data, not an approved catalog yet.
2. Finish host revision/cursor/attempt accounting/error protocol and immutable
   outbox enqueue, including same-key/content precedence and lifetime uniqueness.
3. Implement the **operator-authorized host migration runner**. The SQL file is
   input, not approval. Bind approval to owner, versions, migration digest,
   candidate code and recovery plan; verify quiesced backup/restore, apply a
   transactional migration, record schema_migrations. Keep ADR-0013's complete
   previous/current compatibility window. Wasm must not execute schema DDL.
4. Complete prepare -> DB marker -> exact journal publication recovery;
   runtime/engine opt-in ABI0.2 admission and offline recorded-result replay;
   shared real/mock crash/backup black-box tests; independent exact-head review,
   green CI and merge. External dispatch remains Phase 2.
