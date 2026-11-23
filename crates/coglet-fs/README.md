# coglet-fs

Clean implementation of `spec/abi.md` filesystem primitives. No legacy code.

## API and integration boundary

- `Snapshot::from_files` is a **trusted** bootstrap/restore constructor, not guest
  code admission. Immutable snapshots share an `Arc` tree and file contents;
  `transaction()` forks it, mutations copy the index and touched content, and
  dropping a transaction aborts. `commit()` returns an immutable snapshot; it
  does **not** itself make a runtime step durable.
- Guest adapters must route **every** path operation and descriptor mutation
  through `Transaction` methods. Do not expose `from_files` or host methods to
  guests, or turn a descriptor into unrestricted host file access. This crate is
  not a WASI descriptor-table implementation. All guest path entry points enforce
  ABI path grammar; forbidden paths/mount mutations return `PermissionDenied`
  (the WASI adapter maps it to permission errno). Regular files only; directories
  and roots are implicit. `files()` iterates absolute paths in UTF-8 byte order.
- `/code` is read-only; `/logs` allows create and append only, except
  `/logs/events.jsonl`, which is guest read-only. `/data` is private read/write,
  except guest-read-only `/data/inbox.jsonl` and `/data/runtime/heard.cbor`.
- Trusted boundary methods are distinct: `debug_write`, `host_append_inbox`,
  `host_replace_heard`, `host_append_log`. The runtime must authenticate/serialize
  boundaries, validate **typed schemas and semantic transitions** (including
  canonical JSONL, inbox index/id dedup, heard monotonicity and origin), record
  exact before/after bytes, and retain boundary changes on guest abort. The FS
  preserves those bytes, never reserializes or invents protocol messages.
- Snapshot ABI hashes are SHA256 of core-deterministic CBOR mapping absolute
  paths to lowercase file hex, including all mounts. Blob IDs are SHA256 of raw
  content. Git OIDs are separate and must never substitute for ABI hashes.

## Disk checkpoint

`DiskStore::open` requires a runtime-owned private directory (or an existing
parent for a new directory), on a filesystem supporting atomic same-directory
rename and directory `fsync`. It is not hardened against hostile host symlink
writers. `flush(snapshot, expected_current_hash)` uses an interprocess lock plus
an exact expected-current compare-and-swap (`None` means no checkpoint).

1. Write immutable raw-content blobs to temporary files, fsync each, rename to
   its content address, then fsync the blobs directory.
2. Write a complete path-to-blob index, fsync, rename under the ABI tree hash,
   then fsync the trees directory.
3. Write/fsync a temporary `CURRENT`, atomically rename it over the previous
   pointer, then fsync the root directory. No per-file publication is visible.

Recovery follows only `CURRENT`, verifies every referenced content digest and
path and recomputes the ABI hash. Unreachable blobs/trees/temporary files are
ignored; no automatic GC or WAL truncation is performed. Corruption fails closed.
An error **after pointer publication** is an uncertain commit: recover the
identity, do not assume rollback or blindly re-execute the guest.

This checkpoint primitive does **not** implement the runtime's fsynced WAL,
input-consumption, journal, outbox, step-identity or effect-dispatch transaction.
The runtime must commit these together before dispatch and reclaim its WAL only
after checkpoint and metadata durability. `fd_sync` must not call step commit.

## Git checkpoint

`GitStore` requires an initialized private Git repository and Git supporting
`core.fsync=all`/`core.fsyncMethod=fsync`. It writes raw log blobs, recursive trees
and a deterministic commit, then atomically CAS-updates `refs/heads/logs`. It
never touches the index or worktree. Private `/data` and `/code` are filtered
**before writing any objects**, even unreachable ones. No network operation is
performed. Logs should be batch-checkpointed by runtime policy, not per step.

Disk and Git checkpoints are separate operations, not one cross-store atomic
transaction; local durability is authoritative. Git-ref publication failure is
likewise reconciled by reading `logs_head`, not by assuming rollback.

## Verification

```sh
cargo test -p coglet-fs --locked -- --nocapture
cargo clippy -p coglet-fs --all-targets --locked -- -D warnings
```

The suite has 24 passing tests plus one ignored subprocess entry point (executed
by the crash tests). Named acceptance tests include:

- `stable_hash_across_independent_processes_and_insertion_order`;
  `abi_tree_hash_matches_frozen_trace_snapshot_goldens` checks 29 valid frozen
  trace initial hashes.
- `disk_crash_at_every_flush_point_recovers_old_or_new_never_mixed`: 21 real
  process-exit crash sites in a replacement checkpoint.
- `disk_first_checkpoint_crashes_recover_none_or_complete_new_tree`: 24 real
  process-exit sites for the first checkpoint.
- `git_crash_at_every_object_and_ref_point_selects_complete_old_or_new`: 6
  process-exit sites, exact old/new commit selection, complete log contents and
  `git fsck` on every recovered repository.
- `logs_allow_only_create_append_never_overwrite_or_move` and
  `code_is_read_only_existing_and_absent` test all guest mutation methods.
- `every_frozen_path_forbidden_fixture_through_real_guest_and_boundary_apis`
  rejects all 34 `path-forbidden-*` fixtures; another test enumerates all C0/DEL
  controls and namespace escape forms.
- `reserved_inbox_heard_events_reject_guest_create_write_rename_unlink` and
  `runtime_heard_boundary_is_readable_hashed_and_survives_guest_abort`.
- CAS contention, corruption/missing blobs, I/O errors, private Git object
  exclusion and symbolic-ref isolation have additional tests.

Process-crash injection verifies publication boundaries with the OS cache still
present; it is **not** a claim of physical power-loss or hardware qualification.
The fsync ordering is explicit for a durable filesystem. Full runtime journal /
engine wire-conformance qualification remains with those implementations.
