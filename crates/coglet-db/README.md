# Private native DB substrate — milestones A and B

This is the native admitted-unit slice of ADR-0013, **not runtime ABI 0.2
support**. Existing runtime and engine admission reject DB guests. The public
native API deliberately has no schema provisioning method and exposes commit only through the milestone B coordinator;
all application work rolls back. Successful commits in tests use `cfg(test)`
helpers, absent from production builds.

Implemented and exercised against bundled SQLite:

- Private host-root database per authenticated owner, hashed directory name,
  restrictive Unix permissions, symlink checks, exclusive owner lease, foreign
  keys, WAL and synchronous FULL configuration.
- Immutable typed catalog IDs pin exact SQL, positional parameter/result types,
  result names, table access, mutation target and total order in a deterministic
  CBOR/SHA256 digest. Conservative single-table reads prove a nonnullable unique
  key against the live schema and effective BINARY ordering. Partial, expression
  and nullable unique indexes cannot prove a tie-break key. Separate read/write
  grants and SQLite authorizer isolation protect schema/host metadata.
- Native `Database::admit` checks executing code against approved code hash,
  exact live schema version/fingerprint, schema range and catalog digest. It owns
  the grant/intent-kind snapshot for the entire unit. This is host configuration,
  not guest-provided identity or approval. Runtime manifest admission stays closed.
- Host-only admission-attempt and revision counters use fixed-width big-endian
  uint64 BLOBs. Every admitted attempt is persisted before a lazy application
  transaction, including no-call/aborted attempts. Revision allocation is checked;
  only test commits currently persist application revisions. Read-only commits
  do not advance revision; duplicate enqueue alone is not a write.
- Stateful authenticated pagination uses 256-bit OS-random opaque tokens, storing
  SHA256 verifiers with owner/statement/full-parameter/catalog/schema/revision
  bindings, position and last-row digest. No snapshot/lock survives between pages.
  Tokens expire at 256 further admitted attempts, including aborts. Catalog,
  schema or committed revision changes conflict; smaller later page caps work.
  Paging and mutations cannot mix in a unit. Whole replies including cursors fit
  requested limits; an unrepresentable first row fails rather than disappearing.
- Explicit uint64 cells use eight-byte big-endian SQLite BLOBs for exact storage
  and ordering through `u64::MAX`; wire CBOR retains native unsigned integers.
  Typed catalog metadata distinguishes these from signed SQLite integer cells.
- Dedicated enqueue retains immutable key/content/original ID and canonical CBOR
  payloads. Same-key/content lookup precedes proposed-ID uniqueness; changed
  content/correlation or reused new-key IDs poison the unit. Current native tests
  consume the actual Factory schema/catalog and cover FK read dependencies.
- Short lazy transactions, sticky poisoning, immediate rollback on rejection,
  optimistic affected-row comparisons, 32-call cap and SQLite progress-handler
  cancellation at the absolute step deadline. Error cleanup releases locks before
  the caller drops the poisoned unit.
- Future-host four-buffer call entry validates widened wasm32 ranges, disjointness,
  profile/shape and capacity before execution. It charges input copies and reserves
  maximum result-production/copy bytes before SQL. Exact nonmaterializing CBOR
  sizing avoids uncharged intermediate reply buffers. Typed failures never leak
  SQLite error messages; nonzero returns leave output untouched.

`cargo test --locked -p coglet-db` runs real SQLite tests, including
`db_isolation_between_coglets`, `fk_integrity_enforced`, `query_results_bounded`,
`outbox_duplicate_idempotency_key_rejected` and `optimistic_version_conflict`.
These native tests do not establish shared black-box runtime/engine conformance.
The already-pushed default-off SDK/Factory repository and supplemental compiled
Wasm probe are retained, not production runtime qualification.

## Milestone B: durable full-step coordinator and crash recovery

`Unit::complete(&Decision, &mut dyn Journal)` is the only production commit path.
It validates the prepared ABI 0.2 `db-step-record` (closed schema, canonical
bytes, hash, revision/schema/catalog/owner against the admitted unit), then:
durably prepares the decision (fsynced file beside the owner DB) -> one SQLite
commit of application writes, revision, cursors and the host-only
`_coglet_markers` row -> idempotent exact journal/files/effects/input
publication -> published flag (opens `dispatch_fence`) -> discard preparation.
Steps with no DB calls use ordinary ABI commit and are refused here.

`Database::recover(&mut dyn Journal)` takes no guest runner. Marker present:
roll forward the exact prepared decision. Marker absent: authoritative abort via
`recovery_abort_record`, the ABI 0.2 `db_recovery_abort` overlay (retains calls,
events and boundary; drops files/effects; `db_after=db_before`; new record hash),
published before the preparation is deleted. An unreadable DB or journal is a
`held:` error, never an inferred abort. A missing preparation with an unpublished
marker, hash/identity mismatch, torn preparation or duplicate preparations write
a durable `corruption.halt` and return `corruption:`; admission, recovery and
dispatch then refuse with `halted:` until an operator repairs the owner. Admission
and `dispatch_fence` also refuse while a prior decision is unreconciled.

Tests kill real child processes (`process::exit(137)` at each crash point, no
destructors) and reopen the same SQLite files:
`restart_rolls_forward_when_marker_present`,
`restart_aborts_authoritatively_when_marker_absent`,
`crash_before_prepare_leaves_nothing_to_recover`,
`crash_during_recovery_publication_is_idempotent`, the corruption halts, and a
guest-run counter proves recovery never reruns the guest. `txn_rollback_on_failure`
still passes. The journal is a trait: the real journal, runtime/engine wiring and
admission are milestone D, so runtime admission stays closed and nothing outside
this crate can commit yet. Dispatcher mutations will use the crate-private
`Database::dispatcher_mutation` (same fence, same revision, owner lease).

## Remaining serial milestones (not waived)
- C: operator-approved forward migrations, digest/owner/code/schema/recovery-plan
  bindings, quiesced WAL-safe backup, isolated verified restore and previous/current
  schema compatibility floor.
- D: runtime/real engine/mock opt-in manifest/import admission, invocation-ordered
  call journals and offline recorded-result replay without live DB/catalog/cursor
  access; all seven original shared acceptance cases and recovery/backup controls.

Host roots must remain private and trusted. Path checks are not a sandbox against
another principal already able to mutate the root. Database size quotas and SQLite
connection limits require explicit deployment policy. Catalog construction is not
operator migration approval. External dispatch/reconciliation remains phase 2.
