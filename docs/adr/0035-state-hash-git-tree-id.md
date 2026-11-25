# ADR-0035: State hash is the git SHA-256 tree id

- Status: Proposed
- Date: 2026-11-25
- Authors: David Bloomin (design), Coglet orchestrator
- Related PR: #179; implementation card `coglet: state hash = git SHA-256 tree id`
- Supersedes / superseded by: amends [ADR-0006](0006-coglet-abi.md) and the
  `spec-v0` ABI frozen by [ADR-0009](0009-spec-v0-freeze.md); supersedes the tree
  identity clause of [ADR-0031](0031-manage-port-and-filesystem-patches.md) §2.
  First stage of [staged unforged integration](../cogent-design/unforged-staging.md).

## Context

[Coglets on unforged](../cogent-design/unforged.md) describes running coglets on
unforged, where a coglet's state is a git tree. [Staged
integration](../cogent-design/unforged-staging.md) defers almost all of that work,
with one exception: the filesystem state hash.

Today the state hash is defined in `spec/abi.md` (Journal and trace):
`tree_hash = lowercase SHA256(core_deterministic_CBOR(snapshot))`, over the
path-to-hex map of `/code`, `/data` and `/logs`. `spec/hub-protocol.md` restates
that definition for `debug.read.snapshot_hash` and `debug.write.expected_hash`.
It is computed in three places: `Snapshot::tree_hash` in
`crates/coglet-fs/src/lib.rs` (returns `String`), `Snapshot::tree_hash` in
`crates/coglet-runtime/src/filesystem.rs` (returns `Result<String>`), and
`digest()` in `spec/tests/test_abi.py`, which the fixture generators use.

Every trace, golden fixture and conformance case recorded from now on bakes in
that definition. Changing it later means regenerating or converting all of them.
Changing it now makes every future state hash a git tree id that any git tool,
and unforged, can check.

The definition is part of `spec-v0`, which ADR-0009 froze on 2026-11-23, and
`spec/conventions.md` forbids changing a field's meaning in a minor version, with
no implicit 0.x exception. So this is a breaking change and is versioned as one.

## Decision

Keep coglet and unforged separate. Change only the definition of the coglet
filesystem state hash: it becomes the git tree object id, in SHA-256 object
format, of the snapshot laid out as a directory tree.

- Each snapshot path becomes a tree path with the leading slash removed
  (`/data/count` becomes `data/count`).
- Every file is a blob with mode `100644`. Directories are trees with mode
  `40000`. No other modes exist. The implicit roots `/code`, `/data` and `/logs`
  appear only if they contain a file; git has no empty trees inside a tree.
- Entries are ordered by git's rule (byte order, with tree names compared as if
  followed by `/`).
- Object ids are 32 raw bytes inside tree objects and 64 lowercase hex characters
  everywhere else.
- The empty snapshot hashes to the SHA-256 empty tree id.

**Two new ABI path rules**, so that every valid snapshot maps to exactly one tree.
The ABI already forbids symlinks, hardlinks, device nodes, control characters,
backslash, and empty, `.` and `..` components. It adds:

- no path component equal to `.git`, compared case-insensitively (git's index
  and checkout refuse such entries; `GitStore` already rejects them for logs);
- no path that is both a file and a directory. Both crates already refuse this
  in code; the ABI now says so.

**Fields whose values change meaning**: every filesystem state hash, which is
`initial_hash`, `final_hash`, `before_hash`, `input_hash` and `output_hash` in
the ABI trace, step and boundary records (including the 0.2 database records in
`spec/messages/database.cddl`); the filesystem `state_hash` in conformance
harness and admission responses; `snapshot_hash` / `expected_hash` on the
router protocol's snapshot read and write; and the code tree hashes that the
manage protocol in PR #164 builds on the same algorithm (`manage.patch`
`base` and `revision`, status `live_tree`, and the build record's
`source_tree`). The game-world `state_hash` in `spec/messages/sandbox.cddl` is
not a filesystem hash and does not change.

A **code tree hash** is the same function applied to the snapshot restricted to
`/code`: the root tree holds one `code` entry, and an empty code tree is the
empty tree id. It is not the id of the `code/` subtree.

**Versioning.** ABI trace, step and boundary records move from `"0.1"` to
`"1.0"`, and database records from `"0.2"` to `"1.1"`, so the version string
still tells a database record from a plain ABI one, as
`spec/capabilities/database.md` relies on today. Manifests move with them
(`manifest.schema.json` `"0.1"` to `"1.0"`, the database manifest `"0.2"` to
`"1.1"`), so one ABI version still names both. A `spec-v0` verifier rejects
the new hashes by design; there is no dual-hash window.

On the router protocol, the hub `debug.*` 0.1 methods are not redefined in
place. The new definition arrives with the manage protocol of ADR-0031:

- **This ADR is accepted before PR #164 merges, and #164 adopts the git tree id**
  for `snapshot_hash`, `expected_hash` and its code tree hashes, so manage 1.0
  is defined once, with this hash.
- If #164 merges first anyway, its CBOR definition stands for 1.0, and this
  change ships as the next router-protocol major version instead. A meaning is
  never changed inside 1.0.

**One Rust implementation**, with no git binary at runtime and no new heavy
dependency (`sha2` is already used). `coglet-runtime` depends on `coglet-fs`
only with its `native-engine` feature, so the function lives where both always
reach it (making that dependency unconditional, or a small shared module).
Each `tree_hash` keeps its name and its own return type. The Python reference
in `spec/tests/test_abi.py` gets the same algorithm, so the fixture generators
regenerate by re-running.

**Stored trees.** `DiskStore` names persisted trees by their state hash and
refuses a tree whose name does not match. On first load after the upgrade it
accepts a tree whose name matches the old CBOR digest, then rewrites it under
its git tree id, once. Restart and resume ([ADR-0027](0027-daemon-restart-resume.md))
keep working across the upgrade. A coglet resumed across the upgrade seals its
old trace and starts a new one at the upgrade boundary; hashes inside a trace
are never rewritten, so one trace never mixes the two definitions.

Spec text changes: `spec/abi.md` (Journal and trace, where the definition lives,
and the path rules), `spec/hub-protocol.md` (the restated definition, pointed
at the ABI), the CDDL and JSON Schema descriptions where they describe the hash,
and a cross-reference in `spec/conventions.md`. The comment in
`crates/coglet-fs/src/git.rs` that says git object ids are not ABI tree hashes
is updated; the logs-only `GitStore` is unchanged.

### What does not change

- `record_hash`, `code_hash`, `manifest_hash` and every other hash that is not a
  filesystem state hash keep their current definitions.
- The trace and journal records keep their fields; only the values listed above
  change meaning.
- No dependency on unforged, no git binary at runtime, no change to how code
  reaches a runtime (ADR-0031's `manage.patch`), to the adapter protocol, the
  router, step semantics or effect staging.

Matching unforged is limited to this tree. In the full design the root of a
coglet's commit also holds `manifest.cbor` and `out/`, so a later stage that
adopts it hashes the ABI snapshot as its own subtree; this ADR does not promise
that a stage-1 state hash equals a later root tree id.

## Alternatives

- **Adopt the full unforged design now** (coglet = ref, step = commit, patch =
  merge, router = unforged repository). Rejected for now: it would rewrite the
  journal, trace format, the router's storage and how code reaches a runtime,
  and unforged is not ready for the critical path.
- **Keep the CBOR state hash and convert later.** Rejected: every recording made
  in the meantime would need conversion or regeneration.
- **Add a separate `tree_id` field next to the CBOR hash** (additive, so a minor
  version). Rejected: two state hashes in every record forever, and every
  verifier checks both.

## Consequences

- Every fixture and recorded trace whose state hashes change is regenerated by
  re-running, never by hand-editing: `spec/fixtures/`, `conformance/abi/*.json`
  and `conformance/hub/*.json`, the visualizer trace fixtures under
  `tools/visualizer/tests/fixtures/`, and the crates' test data.
- This change blocks new golden fixtures until it lands, so fixtures are recorded
  once against the new definition.
- Old recordings with CBOR state hashes do not verify under the new definition;
  none are retained as compatibility inputs. Stored trees are upgraded once, as
  above.
- PR #164 has to adopt the git tree id before it merges, or this change waits
  for the router protocol's next major version (see Versioning).
- Later stages (proof test, centre cold path, router on unforged, one effect
  path) each need their own ADR and start only after the previous stage passes.

## Verification

- `state_hash_matches_git_sha256_tree`: for a set of snapshots (empty, one file,
  nested directories, names that sort differently as trees versus files, binary
  contents), the computed id equals the id real `git` produces in a repository
  initialized with `--object-format=sha256` (via `git mktree` or `git
  write-tree`). The test may use the git binary; production code may not.
- `state_hash_rejects_unmappable_paths`: a snapshot with a `.git` component, or
  with a path that is both a file and a directory, is refused before hashing.
- `state_hash_single_implementation`: `coglet-fs`, `coglet-runtime` and the
  Python reference return the same id for the same snapshot.
- `disk_store_upgrades_cbor_named_trees`: a store written under the old names
  loads once and is rewritten under git tree ids, and a resumed coglet starts a
  new trace rather than extending the old one.
- `code_tree_hash_is_restricted_snapshot_id`: a code tree hash equals the state
  hash of the snapshot restricted to `/code`, and the empty code tree is the
  empty tree id.
- `cli_build_run_replay_equal_hash_list_and_inspect` and all conformance suites
  pass with regenerated fixtures, on both WAMR and Wasmtime where applicable.

## Approval

Pending independent review and the accepting PR.
