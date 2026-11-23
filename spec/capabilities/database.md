# Private database capability — ABI 0.2 extension (proposed)

Decision: [ADR-0013](../../docs/adr/0013-factory-db-capability.md),
[PR #41](https://github.com/softmax-universe/coglet/pull/41).
Imports [common conventions](../conventions.md) and [ABI 0.1](../abi.md) unchanged
except for the explicitly versioned additions here. MUST/SHOULD are normative.
This extension is not part of tag `spec-v0`. No implementation is supplied.

## Admission and operation binding

A DB guest pins `db-manifest` (version `0.2`), whose `database` field contains
`schema_min`, `schema_max` (inclusive, min <= max) and `catalog_hash` (ABI hash).
The runtime rejects before instantiation unless it supports the extension, the
live schema is in that range and the approved immutable catalog digest matches.
The module may import `coglet_v0.db` only under that manifest; 0.1 admission rejects
that import. Existing handle/allocator, mounts, counters and all other imports
are unchanged. Pin code, grants, schema and catalog for the entire step. Host
revocation affects the next admitted step and dispatch admission, not past replay.

| Grant | Request binding | Response binding |
|---|---|---|
| `db.read` | `database:db-statement` with mode=read | `database:db-page` or `database:db-failed` |
| `db.write` | `database:db-statement` with mode=write; `database:db-enqueue` | `database:db-page`, `database:db-enqueued` or `database:db-failed` |

These are built-in ABI operations, not surface advertisements or hub methods.
This version-pinned table is the DB extension catalog; the exhaustive **v0**
`bindings.json` is deliberately unchanged. A `db.write` grant does not imply
`db.read`; a read statement needs the latter. Enqueue additionally requires the
current grant for the intent kind in a trusted dispatcher catalog. The host binds
the owner to the authenticated running coglet; there is no owner/database argument.
Every coglet has a separate private logical database. No guest credential, DSN,
path, SQL text or cross-owner table reference is accepted. Debug/archive/backup
access requires separate authorization; possession of a cursor is not authority.

## Buffer ABI and limits

```text
coglet_v0.db(ptr: i32, len: i32, out_ptr: i32, out_cap: i32) -> i32
```

Arguments are unsigned wasm32 bit patterns. Input is one deterministic-CBOR
`db-request`, at most 65,536 bytes. Output capacity MUST be at least 65,540 bytes:
4-byte unsigned big-endian result length followed by one deterministic-CBOR
`db-reply`, at most 65,536 bytes. Only that prefix is written. Validate both widened
ranges against current memory before access, reject overlap, and copy input before
execution. Invalid pointers/overlap abort the step with ABI invalid_argument;
no partial output. All reads/copies and result production count against aggregate
host_bytes; both the size check and output/budget reservation precede DB execution.
At most 32 DB calls per step (malformed/rejected calls count). The absolute step
wall deadline includes DB work. Cancel/rollback on expiry, never leave a lock open.

Return 0 means a complete typed reply was copied, NOT a durable DB commit. A typed
`db-failed` still returns 0. Return 1 means invalid CBOR/schema or inadequate output
capacity; 2 permission_denied; 3 resource_exhausted; 4 conflict for an already
poisoned unit. Nonzero returns write nothing and poison the DB unit. The journal
records raw `argument_hex` for invalid arguments, otherwise typed arguments, plus
`result`. Only result=0 carries a typed `response`; response bytes are the canonical
encoding of that value. Invalid pointer failures follow ABI failure.details and do
not fabricate a copied argument. Validation order: pointers/profile/shape, grant,
poison state, duplicate/cursor checks, then fresh resource/statement admission.
A duplicate cannot bypass authorization. A previously poisoned unit cannot enqueue.

## Statements and paging

`db-statement` has operation=statement, statement (1..64 ASCII catalog identifier),
mode (read/write), up to 32 positional params, max_rows=1..64, max_bytes=256..65536,
and optional cursor (1..512 UTF-8 bytes). Cells are null, signed-negative int64,
uint64 or UTF-8 text up to 4096 encoded bytes; no blobs, floats or implicit JSON
coercion. The host refuses an unrepresentable DB value rather than truncating it.
The approved catalog binds exact parameter types/count, portable parameterized SQL,
read/write mode, allowed application tables, column names/types and a total ORDER
BY with a unique tie-break key. Bind values, never interpolate SQL/identifiers.
Reject multi-statement SQL, DDL, extensions, PRAGMAs, clock/random functions and
host-owned metadata access at catalog approval. Ordinary application statements
cannot insert/update/delete `outbox`, `schema_migrations` or recovery metadata;
enqueue is the only guest outbox mutation. Schema changes use approved migrations.

`db-page` returns status=page, at most 16 columns and 64 rows, affected (uint64),
revision (the committed revision at step admission), and next_cursor (text or
null). Each row has exactly as many cells as columns; column names are unique.
Encoded **entire reply**, including keys/cursor, MUST fit requested max_bytes and
65,536 bytes; at most max_rows rows. Cell text .size is UTF-8 bytes, not characters.
A row that cannot fit yields resource_exhausted and poisons the unit, never silent
truncation. UTF-8 text/cursor/column byte caps and the encoded-whole-reply cap are
protocol-semantic checks: both schemas intentionally leave text unbounded rather
than confuse JSON character length with encoded bytes. Codec shape acceptance
alone is not DB admission. Writes have no returned rows/columns/cursor; affected is actual count.
Read affected=0. Requests with cursor must be read-only. Cursor identifies the
first not-yet-returned row; next_cursor=null means exhausted, not an empty hidden
page. Never return zero rows with a non-null next_cursor.

A cursor authenticates owner, statement, full parameters, catalog hash, base
revision and last key. It expires after 256 further admitted attempts, including
aborts, and on catalog/schema change. Any intervening committed write (including
outbox receipt/lease changes) advances the private DB revision, invalidating it.
No transaction/snapshot survives between pages. Missing/expired/mismatched cursor
returns typed conflict; it must not restart at page one implicitly. Later page
requests may use smaller row/byte caps. A step that has already written cannot
consume or mint a cursor; later write calls after a cursor-bearing read are also
refused with conflict. Finish paging then mutate in a fresh step. Serial read-only
steps do not advance revision, although they retain commit markers. This favors
bounded consistency over starvation-free scans of a busy store; exported artifacts
are the alternative for long scans.

## Transaction and journal

One lazy host transaction per admitted guest step. All successful statements,
semantic events and enqueues share it. A typed failure or nonzero host return
poisons the whole unit. Further calls return 4; even handle=0 produces an aborted
attempt with failure.kind=guest_return, error.code=conflict. Any ordinary ABI abort
also rolls back the unit. Successful replies remain provisional and stay recorded
on abort. A `db-failed` has status=failed and common error (including conflict,
resource_exhausted, permission_denied, unavailable or invalid_argument as appropriate).

`db-host-call` extends the invocation-ordered `calls` array of `db-step-record`;
ordinary ABI calls are unchanged. `db-step-record` is the same closed record as
ABI step-record except version=0.2, the extended call union and required fields
`db_before`, `db_after` (revision), `db_schema_version` and `db_catalog_hash`.
For abort db_after=db_before; for committed read-only steps they are equal; for a
committed unit with writes/enqueues db_after=db_before+1, rejecting uint64 overflow.
Duplicate enqueue alone is not a write. Revision allocation is serialized, so it
is known when preparing the step. Dispatcher mutations also advance revision.
A record's db_before need not equal the previous step's db_after when dispatcher
work intervened: those observations are recorded, not inferred by offline replay.
`db-trace` extends ABI trace with version=0.2, `records` of db-step-record,
`initial_db_revision` and `final_db_revision`. `db-boundary-record` is the unchanged
ABI boundary-record shape with version=0.2; a 0.2 trace's trailing_boundary uses
that rule, preserving header/record version equality. These are archive observation bounds,
not permission to access a live DB. Record hashes use the existing ABI hash rule
on the complete extended record; ordinary filesystem tree hashing is unchanged.

Replay checks the exact call order, argument bytes and recorded response/return;
it returns the recorded result without connecting to SQLite/PostgreSQL, validating
a cursor against current state, resolving a statement catalog, or dispatching.
Replay aborts on a divergence; it never falls back to live reads. Replay also uses
the recorded schema/catalog/grants, not current ones. `events` is Factory semantic
history, never a second copy of journal calls or a journal replay input.

## Outbox lifecycle and recovery

`db-enqueue` includes operation=enqueue, intent_id (common action-id),
idempotency_key (common correlation-id), kind (capability-name), payload
(profile-map), correlation_id. Guest intent IDs are unique across the private DB
lifetime. Identity equality includes kind, payload and correlation_id; the key is
scoped to owner. Same key/content returns status=enqueued, original intent_id,
duplicate=true, even if a retry supplies a different proposed intent_id. Lookup
by existing key/content takes precedence: the proposed intent_id is ignored for
an identical retry, even if it already belongs to another key. Only a **new key**
checks intent-ID uniqueness; a reused intent_id then yields conflict. Changed
content under an existing key yields conflict regardless of proposed ID. Either
conflict poisons the unit. First insert returns duplicate=false. Keep key/content/ID
and terminal receipts through DB lifetime (including backup restores); pruning
needs a new approved epoch and reconciliation plan, not a silent TTL.

Factory owns inserts; host owns dispatch status. States are pending, dispatching,
uncertain, succeeded, failed and cancelled. Only journal-published committed
intents are eligible. Lease expiry permits reconciliation, NOT unconditional
redelivery. Each attempt retains the original key and payload. Terminal success,
failure or cancellation is durable before one inbox `db.outcome`: `db-outcome`
contains original intent_id, idempotency_key and correlation_id, terminal status,
and result (profile-map) on success or common error otherwise. Runtime/hub
materialization uses the existing authorized `hub.inbox` path and `runtime.inbox`
wake, not a new runtime event/audit method. Enclosed source is the authenticated
dispatcher/hub; correlation IDs must match payload/envelope/intent. Atomically
retain a delivery ledger and stable inbox ID at delivery; retry the same append,
never mint a new outcome. Uncertain is nonterminal and has no synthetic success.
If a terminal intent is duplicated the original receipt is reconciled, not sent
to the guest twice. Revocation denies new external dispatch and records an
appropriate failed/cancelled terminal; an already uncertain external effect must
still be reconciled, not mislabeled cancelled. Destination authority, quotas and
expiry are rechecked by the dispatcher and cannot be acquired through SQL.

The prepare -> DB marker -> exact journal publication -> dispatch fence and full
crash matrix in ADR-0013 are normative. Markers are host-only and bind
(coglet, trace_id, step) to the prepared record hash and resulting revision. A
failed attempt has no successful DB marker. Before next admission recover any
uncertain decision; missing marker means abort only after authoritative recovery,
never because of a network timeout. A committed marker MUST roll forward its
prepared successful decision. Missing prepare/hash mismatch is corruption and
halts the owner. Do not replay guest code to reconcile a DB commit. Dispatch and
snapshot mutations share the same per-owner serialization/commit fencing.

## Migration, backups and portability

ADR-0013's digest-bound explicit approval, SQLite backup/restore manifest and
compatibility window are normative. Approval includes schema versions, migration
hash, candidate code/owner and recovery plan; db.write and ordinary promotion do
not grant it. Current and previous deployed code must both support the schema
through all probation AND one later successful promotion, whichever is later.
Incompatible/destructive changes require separately approved maintenance; automatic
code rollback checks schema range and never restores old data over external work.
PostgreSQL and its backup/qualification tooling are deferred, not emulated by
SQLite-specific SQL. No live schema engine or migration implementation is added here.

## Verification and qualification boundary

`spec/fixtures/database/cases.json` selects rules and golden deterministic bytes.
`spec/tests/test_database.py` validates CDDL/JSON, bounds and temporal vectors.
`conformance/database/` contains executable engine-seam adapter timelines; its
README defines the input setup and observed outputs. Scripted reader tests exercise
the actual harness including wrong-result negatives. These are case-contract
checks, **not** real SQLite/engine or dispatcher conformance. Phase 1 must prove
private ownership/grants, rollback/poison, FK/version checks, budget cancellation,
all crash points and backup restore using its real engine seam. Phase 2 must prove
restarted dispatcher uncertainty, stable-key dedup and inbox outcome delivery.
Both real and mock adapters must pass the same suite before claiming support.
