# Native DB maintenance — milestone C

This is a host library, not a guest capability, CLI approval service or runtime
qualification. Runtime/engine ABI 0.2 admission remains closed until milestone D.
No spec or ADR gate is waived.

## Approval and migrations

`Database::migrate` requires a separate `OperatorApproval` collaborator. Its
implementation must authenticate an explicit human/operator **migration** approval
for `Migration::digest()`. There is intentionally no built-in accept-all authority,
boolean approval flag, `db.write` shortcut or ordinary promotion shortcut. The digest
binds authenticated owner, exact SQL, source/target schema versions and fingerprints,
previous/candidate code hashes and ranges, and the recovery plan. Approval service
integration is a host responsibility; passing a guest-supplied verifier is invalid.

The existing owner lease and mutable borrow serialize maintenance with steps and
host dispatcher mutations. Call recovery first: active transactions, pending
preparations, unpublished decisions, corruption halts and isolated restores refuse
maintenance. Each request uses fresh snapshot/trial paths. No snapshot is deleted
or overwritten: keep the returned known-good recovery snapshot and earlier
known-good snapshots through probation and a newer verified restore. Longer
retention is the owner's private data policy.

Before live SQL: take an online SQLite backup, restore it into a new isolated root,
verify checksums, integrity/FKs, marker alignment, required replay/reference checks,
and close/reopen it. Apply the migration to that isolated copy; validate the exact
target schema and reopen again. Only then apply the same SQL to live storage.
Application DDL, `schema_migrations`, full approved request, deployment floor,
`user_version` and cursor invalidation commit together in one SQLite transaction.
An error rolls back, leaving old code/schema eligible; it never restores old data
over effects. Failed trials and recovery snapshots remain private for diagnosis.

This initial implementation intentionally admits only additive table/index DDL
(and index rebuilds), including initial outbox table/index constraints. It denies
ALTER/DROP, triggers/views/virtual tables, arbitrary row DML, PRAGMAs, attachment,
transaction control and host metadata changes. It creates the host-owned
`schema_migrations(version, applied_at, migration_hash)` itself. For an initial
Factory deployment, package the approved application DDL separately from that
host-owned table definition; fingerprint the complete resulting schema including
the host-created migration-history table. Destructive/incompatible maintenance has
**no automatic path** here and needs a separately approved maintenance mechanism.

## Compatibility floor

`CompatibilityFloor` persistently pins previous/current code hashes and inclusive
schema ranges. Both must support the target. Native admission checks the floor as
well as existing exact pins. `check_code_compatibility` checks promotion/rollback
without changing data. This milestone has **no floor-release API**: subsequent
migrations cannot replace either pin. That is deliberately stricter than the
probation-plus-one-successful-promotion minimum, not a timer-based weakening. Future
promotion integration must preserve that minimum before adding a release path.

## Snapshot and restore contracts

`Database::snapshot` uses SQLite's online backup API, so committed WAL-only rows are
included without copying a live database file. The new private archive holds:

- database bytes (including schema history, counters, markers, outbox state/receipts);
- a checksummed recovery bundle with journal head, approved catalog digest, exact
  retained committed decision records/staged artifacts and immutable blob bytes;
- manifest with owner, schema version/fingerprint, revision/admission counters,
  database and recovery-bundle digests.

Keep `Snapshot::manifest_digest` outside the archive in trusted host storage;
checksums are integrity, not operator authorization. Archive roots are trusted host
paths, never guest-supplied. Directories/files use 0700/0600 on Unix. The owner lease
is the supported writer boundary; arbitrary principals able to alter the host root
are outside the sandbox.

`RestoreVerifier` is required at snapshot and isolated restore. It must check the
**authoritative journal head and catalog**, referenced blob closure, staged
files/effects/input coverage, and representative recorded-result replay. The library
itself checks blob hashes and a one-to-one match of every retained DB marker with
its canonical, hash-valid owner/step/catalog/revision decision. It cannot interpret
the host's opaque staged-artifact format or execute a guest engine. Tests verify
nonempty canonical recorded observations/artifact coverage with a strict fixture
verifier; full runtime/engine replay integration is D, not claimed by C.

`Database::restore_isolated` only creates a **new root**. Before copying any data it
fsyncs a durable `restore.isolated` hold. Integrity, foreign keys, manifest boundary,
marker/journal alignment and the verifier must pass. It closes and reopens SQLite
before returning a report. Even a successful report does **not** activate the
restore: admission/dispatch/migration remain held across reopen. There is no API to
remove that hold or replace a live owner, preventing a snapshot rollback over
post-snapshot external work. Production recovery activation needs a separate
reconciled maintenance decision; deleting a hold file is not that decision.

## Executed acceptance tests

`cargo test --locked -p coglet-db` exercises real SQLite, not SQL-file existence:

- `approval_binds_every_field_and_ordinary_grants_are_not_approval`
- `approved_forward_migration_records_digest_and_persists_floor_on_reopen`
- `failed_trial_schema_or_sql_keeps_live_code_schema_and_data`
- `incompatible_previous_or_candidate_is_rejected_before_backup`
- `floor_cannot_be_released_by_a_second_approved_migration`
- `wal_safe_snapshot_restore_and_reopen_remain_isolated`
- `snapshot_restores_nonempty_markers_receipts_and_immutable_artifacts`
- `approved_additive_outbox_ddl_allows_constraints_but_never_row_mutation`
- `archive_corruption_owner_mismatch_and_failed_replay_are_rejected`
- `pending_decision_transaction_marker_or_blob_mismatch_blocks_snapshot`
- `foreign_key_corruption_refuses_backup`

Existing A/B real-SQLite transaction, private-owner, poison/budget and killed-process
recovery tests remain active. Runtime admission, a production operator identity
adapter, actual guest replay and dispatcher destination reconciliation are not
certified by these library tests.
