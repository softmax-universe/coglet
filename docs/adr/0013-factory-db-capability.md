# ADR-0013: Factory database capability and recovery boundary

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet contributors
- Related PR: [#41](https://github.com/softmax-universe/coglet/pull/41) (published before spec edits)
- Supersedes / superseded by: extends ADR-0006; does not change spec-v0

## Context

The [Factory proposal](../design/factory-cognitive-architecture.md) needs a private
relational store without putting a database engine or credentials in Wasm. The
frozen ABI 0.1 has no synchronous observation import; pretending `emit` returns a
query result would violate it. Database and journal commits are different durable
resources. An external action must not escape a guest step that later aborts.

## Decision

Add an explicitly opted-in **ABI 0.2 DB extension**, specified by
`spec/capabilities/database.md` and the `database` message family. Keep frozen 0.1
records, imports, manifests and fixtures unchanged. A DB-enabled deployment pins
0.2 in its manifest and journal; hosts lacking this extension reject it before
instantiation. This is an additive version, not an implicit upgrade of 0.1.

### Calls, isolation and authorization

A synchronous `coglet_v0.db` import exchanges deterministic CBOR request/result
buffers. Read and write calls use host-approved statement IDs with positional
parameters, never guest SQL, credentials, file names or connection strings. A
pinned statement catalog binds SQL text, parameter types, read/write mode, table
access and result ordering; catalog changes are approved configuration changes.
Every coglet owns one private logical database selected by authenticated running
identity, not an argument. `db.read` and `db.write` are separate per-coglet grants;
write does not imply arbitrary schema access, another coglet's database, or the
host-owned recovery/outbox-dispatch metadata. Inter-coglet exchange uses the hub.

A step opens at most one short DB transaction, lazily, and releases it before
returning to the scheduler. Calls see their own writes. No handle, transaction or
lock survives a step. Guest abort or a rejected DB call rolls back the complete
DB unit; errors poison the unit, so a guest cannot ignore a failed write and
commit a partial unit. Results are bounded by rows, columns, cell bytes, total
encoded bytes, call count and the existing aggregate host-byte/wall budgets.
Read paging uses an opaque authenticated cursor binding owner, statement,
parameters, catalog and DB revision; revision changes invalidate it with conflict.
There is no held database snapshot/transaction between pages. Repository queries
must order by a unique stable key; a stale page is restarted, not silently mixed.

### Durable outbox and commit decision

Factory cognitive changes, semantic events and outbox inserts share the private
DB transaction. The Factory owns intent creation; the host dispatcher owns delivery
leases, attempts and receipts. A dedicated import operation enqueues an intent;
ordinary statements cannot modify dispatch/recovery metadata. Keys are scoped to
(coglet, idempotency_key), retained for the database lifetime. Identical retries
return the original intent ID without adding work; different kind/payload under
the same key returns conflict and aborts the DB unit. Dispatch uses that same key
and immutable payload on every attempt. Destinations must reconcile uncertain
receipts; non-idempotent uncertain actions are held, not blindly resent. There is
no exactly-once execution claim.

The host first durably prepares the complete successful step decision, including
journal bytes/hash, files, input consumption and staged ordinary ABI effects.
Only then does it commit the DB transaction with a host-owned marker binding
(coglet, trace_id, step), prepared journal hash, catalog and resulting revision.
This DB commit is the irreversible decision point. The host then publishes the
prepared step/journal commit idempotently. Dispatch of either ordinary ABI effects
or Factory outbox intents is fenced until that exact journal hash is published.
A read-only DB step also uses this marker; a step with no DB calls uses ordinary
ABI commit. The preparation is a recovery artifact, not a second execution journal.

| Crash point | Required recovery |
|---|---|
| Before durable prepare | Roll back DB; no dispatch. Recover ordinary ABI attempt admission; no automatic live rerun. |
| After prepare, before DB commit | Marker absent after authoritative DB recovery: discard preparation, roll back, durably record an aborted attempt; no effects. |
| DB commit uncertain | Query the marker, not guest code. Inaccessible DB means hold; never infer abort from timeout. |
| After DB commit, before journal publication | Matching marker: roll forward the exact prepared journal/files/input consumption. Never rerun or turn the committed DB unit into an abort. |
| After journal publication, before dispatch | Reconcile marker/hash, resume eligible pending intents. |
| Dispatch happened, receipt absent | Query/reconcile destination with original key; retain uncertain status until resolved. |
| Receipt persisted, guest outcome not yet delivered | Idempotently append one later inbox outcome with original correlation and intent identity; retained delivery ledger prevents a second append. |

A missing prepare with a committed marker, mismatching hash or divergent step
identity is corruption: fail closed for repair. Never restore an old DB over
post-snapshot external effects. Serialize DB commit/recovery and guest admission
per coglet. Before a new step, finish reconciliation of the previous decision.

### Journal and replay

DB call arguments and exact result bytes (including errors/duplicates/pages) are
ordered entries in the ABI 0.2 step journal. Replay validates call sequence and
arguments and supplies recorded bytes without opening a DB, consuming a cursor,
applying migrations or dispatching intents. Recovery is not replay. The Factory
`events` table holds meaningful cognitive decisions only; it is not a copy of the
journal or an alternative event-sourcing authority. The journal owns execution
history; SQL owns live cognitive state. Snapshots carry both recovery positions.

### Migrations, snapshots and compatibility

Use SQLite's online backup API (or a checkpointed, quiesced consistent snapshot),
not copying a live database file without its WAL. Quiesce guest steps and dispatcher
receipt mutations at a recorded boundary, reconcile commits, and capture DB,
schema/catalog version, commit markers, journal head, outbox receipts and referenced
immutable blobs in a checksummed manifest. Restore into isolation, validate integrity,
foreign keys, journal/marker alignment and representative replay before use.
Keep the previous known-good snapshot through probation and until a newer restore
has been verified. Retention beyond this recovery floor follows the owner's data
policy; backups inherit database privacy/access controls.

A schema migration requires explicit human/operator approval bound to owner,
old/new schema versions, migration digest, candidate code and a recovery plan.
A `db.write` grant or ordinary promotion approval is insufficient. Trial only on
an isolated backup. Live migration quiesces steps/dispatch, takes and verifies a
recovery snapshot, applies an approved transactional migration, then switches code
at the boundary. Failed migration leaves old code/schema active. Expand/contract
is mandatory for automatic code rollback: **both current and previous deployed
code remain compatible for the entire probation period AND at least one subsequent
successful promotion**, whichever ends later. No automatic destructive migration;
an incompatible change requires a separately approved maintenance plan and disables
automatic code rollback until compatibility is restored. Explicit schema ranges
are checked before promotion and rollback; snapshots are not automatic data undo.

Preserve portable statements, explicit optimistic versions and relational constraints.
PostgreSQL later implements the same logical contract using transactionally committed
markers and backend-consistent backups (with journal/outbox alignment). PostgreSQL
hosting, backup tooling and dual-backend CI are deferred until a separate backend
ADR and implementation qualification; SQLite semantics are not the portability spec.

### Disposition of the proposal's open questions

1. DB protocol/lifetime: opt-in 0.2 buffer import, catalog statements and single-step
   transactions, bounded pages (above and capability spec).
2. Outbox ownership/atomicity: private Factory DB, host dispatcher and marker-fenced
   journal publication; at-least-once delivery with explicit uncertainty.
3. Journal versus events: recorded observations versus semantic cognitive history.
4. Worker capacity/budget policy and Hermes adapter details: explicitly deferred to
   Factory Phase 2/product adapter work. This protocol enforces local budgets and
   authorized intent kinds, but does not decide workforce scheduling or adapter
   internals; those cannot widen grants or bypass the dispatch fence.
5. SQLite backups now, PostgreSQL path later: consistent backup plus aligned manifest.
6. Approval/compatibility: digest-bound approval and the explicit probation/promotion
   compatibility floor above.

## Alternatives

- Shared tables across coglets: rejected; makes identity/grant isolation ambiguous.
- Arbitrary SQL and long-lived transaction handles: rejected; bypasses catalog
  policy and holds locks across event waits.
- Async `emit` disguised as a synchronous query: rejected; violates frozen ABI.
- DB commit before preparing journal, or dispatch before publication: rejected;
  permits unreconstructable cognitive state or effects from an aborted step.
- XA/distributed exactly-once: unnecessary and not portable to external providers.
- Duplicate execution event sourcing in `events`: rejected; two competing histories.

## Consequences

ABI 0.2 is opt-in and requires new host support; existing v0 engines cannot run a
DB-enabled Factory. Implementations must qualify real crash durability, private
identity binding, rollback and dispatch reconciliation before production. Schema,
golden and scripted case-contract tests do not establish those runtime properties.
This ADR contains no database implementation or Hermes adapter internals.

## Verification

The combined PR will register `database` CDDL/JSON messages, valid/invalid goldens
and executable `conformance/database/` engine-seam timelines for bounded results,
paging, abort and duplicate keys. `spec/tests/test_database.py` validates both
schemas, golden bytes, cursor/poison/commit semantics and exercises the actual
harness with a scripted frame reader (case contracts, not a DB implementation).
Run `codegen/run --check`, independent schema cross-checks, protocol tests and the
conformance harness self-test. Independent exact-head review and green CI are
required before merge. Real SQLite crash injection/isolation and destination
reconciliation are Phase 1/2 implementation gates, not claims of this spec PR.

## Approval

Pending independent exact-head review and accepting merge of this PR. A proposed
ADR and scripted fixtures alone do not approve a migration or qualify a runtime.
