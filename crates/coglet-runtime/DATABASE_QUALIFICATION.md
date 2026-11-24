# ABI 0.2 database qualification (milestone D)

## Executable gates

`python3 crates/coglet-runtime/tools/check-database.py` installs the no-WASI
Factory target, runs the native DB controls and full serial native-runtime suite,
strict native clippy, then executes **every** frozen `conformance/database` case
through the official language-neutral harness on both `real` (WAMR interpreter)
and `mock` (shared native engine). Fresh JUnit inventories must equal the corpus
with no failures/skips. `coglets/factory/check.sh`, already called by the required
Rust job, invokes this gate. The Rust and Apple jobs install the no-WASI target
before their first runtime tests, which also compile the actual Factory guest.
Disposable DB roots are canonicalized so macOS's `/var` alias does not violate
SQLite's production `NOFOLLOW` policy; that production policy is unchanged.
Hosted exact-head results remain a landing gate, not a claim from local execution.

| Requirement | Named proof |
| --- | --- |
| Frozen bounded page / authenticated continuation | `db-bounded-page`, `db-paging-cursor`, both wire adapters |
| Whole-unit rollback / ignored conflict | `db-transaction-abort`, `db-conflicting-key`, both wire adapters |
| Lifetime identical-key dedup, including proposed-ID collision | `db-duplicate-key`, `db-duplicate-key-id-collision`, both wire adapters |
| Prepared decision absent/present marker | `db-recovery-no-marker-abort`, `db-recovery-marker-roll-forward`, both wire adapters |
| Durable full service state/reopen | `real_wamr_service_commits_exact_native_record_and_reopens`, `mock_service_commits_exact_native_record_and_reopens` |
| Recovery without guest execution / DB or journal unavailable holds | paired `*_marker_present_recovers_forward_without_guest`, `*_marker_absent_aborts_same_attempt_without_guest`, `*_database_unavailable_holds_and_loses_nothing` in `database_service` |
| Recorded-result offline guest replay | paired `*_replay_uses_recorded_results`, `*_replay_enforces_mixed_call_order_without_database` and recovery-abort negative controls in `database_service` |
| WAL-safe backup/isolated restore with real guest replay, missing-reference refusal | `real_wamr_backup_restore_replays_and_remains_isolated`, `mock_backup_restore_replays_and_remains_isolated` |
| Actual Factory guest/catalog/approved migration, reads/update/event/enqueue/log, duplicate rollback/reopen/offline replay | `real_wamr_runs_actual_factory_guest_end_to_end`, `mock_runs_factory_repository_against_real_database_host` |
| Exact-lossless archive roundtrip, versioned trailing seals, empty successor revision across WAL reopen | Factory tests above plus eight `database_archive` tests; frozen `trace` tests remain required |

The original phase-1 seven properties are distinct from the eight frozen wire
timelines. The shared `tests/support/database_properties.rs` driver now runs each
through both actual adapters, RuntimeService, WAL reopen and SQLite. Named tests
use both `real_wamr_` and `mock_` prefixes:

- `db_isolation_between_coglets`: private paths, wrong-owner refusal before guest
  entry, and own-only query result with a different owner's secret present.
- `rollback_on_failed_transaction`: ignored duplicate failure rolls back a prior
  write, poisons later calls, and releases the write lock.
- `optimistic_version_conflict`: approved conditional update actually affects zero
  rows; prior write cannot commit despite handle=0.
- `fk_integrity_enforced`: real missing-parent insert fails and rolls back all writes.
- `outbox_duplicate_idempotency_key`: actual immutable-key duplicate returns the
  original ID with one durable row; changed-key/content controls are in wire tests.
- `query_results_bounded`: 65 actual rows yield 64 ordered rows and a real cursor,
  with bounded whole reply bytes and no lost row.
- `db_property_replay_uses_recorded_results`: own provisional write is read and
  replayed offline. Every property also reexecutes with the DB physically deleted;
  the separate reply-dependent replay tests exclude canned-success execution.

## Test-only engine-seam translations

`examples/database_adapter.rs` receives requests only; it never reads corpus
expected responses or case names. Each request provisions one private revision-7,
schema-1 SQLite fixture. Its adapter grants are trusted harness provisioning, not
a guest authority or a product authentication boundary.

- The opaque cursor fixture label maps to a real token minted and **committed**
  by an extra read-only step through the selected engine and same private DB.
  The requested second page uses that token in actual guest memory. The outgoing
  token is projected to the corpus label only after successful host execution.
- The documented `force_conflict` optimistic-update shorthand is sent as
  `mode=read` in the corpus. At this fixture seam it maps to the approved write
  statement `UPDATE ... WHERE version=0`, whose actual affected count is zero;
  it is not a fabricated failed response. The stored rows have version 1.
- Common conflict replies retain the actual code/retryability; only the sanitized
  message is projected from the implementation's `conflict` to `Unit aborted`.
- Recovery uses a real durable prepare, marker insertion fault or publication
  failure, disk reopen, and authoritative `recover_database`. Before recovery the
  adapter reads the exact successful preparation. Afterward it checks the actual
  marker/committed record hash or recovery-abort preparation hash against that
  decision. Only then is its real hash projected to the deterministic fixture
  hash alias. The actual verified hash is logged to stderr, never substituted
  into durable records. Dispatch output means eligibility, not external effects.

Backup verification in `tests/support/database_backup.rs` compares the exact
prepared-state digest with the authoritative WAL receipt, checks referenced input
blobs and reexecutes the pinned guest with recorded replies. This is a test
collaborator; product operator authentication and backup release authorization
must remain separate. Restores retain the durable isolation hold and never
replace live data or authorize external dispatch.

## Remaining boundaries (not waived)

The explicit borrowed `run_next_database` deployment path pins authenticated
owner, grants, code, live schema and catalog before instantiation. Ordinary 0.1
loaders still reject DB imports. The daemon's generic admission adapter has no
Database/Catalog provisioning context and deliberately refuses 0.2; it is not a
product database deployment API.

0.2 archive step records are retained as exact CBOR content-addressed blobs, not
reencoded from JSON. Archive import validates host transitions, **not** independent
guest execution; `replay_database` is the separate guest proof. Trailing seals use
the manifest-pinned boundary version. Empty archives retain an observed revision
from trusted setup, imported header, or the last step across segment transfer;
an unknown revision is refused rather than fabricated as zero. These are archive
observations, not a fresh live DB query or authority. No frozen spec or
fixture changes, ADR gate waivers, PostgreSQL emulation, external dispatch or
physical worker-success claim are part of these tests.
