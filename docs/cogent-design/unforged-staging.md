# Coglets and unforged: staged integration

Status: proposed. Owner: David. Date: 2026-11-24.

This page decides how much of [Coglets on unforged](unforged.md) to adopt now.
Part of the [cogent design](README.md).

## Decision

Keep coglet and unforged separate for now. Make one change immediately: the
coglet state hash becomes the git SHA-256 tree id of the coglet's files.
Everything else about unforged integration is deferred and staged behind a
proof test.

## Why now, and why only this

The full design (coglet = ref, step = commit, patch = merge, router = unforged
repository) is attractive, but adopting it now would rewrite the journal, trace
format, the router's Git transfer and router storage while M1 and the v0 freeze are in
flight. unforged is also not ready to sit on the critical path: it needs git
2.45+ for reftable, its CAOS cache-key and signature findings are untested, and
it has never run a coglet step.

The state hash is the exception. Every trace, golden fixture and conformance
test recorded from now on bakes in its definition. Changing it later means
regenerating or converting all of them. Changing it now is a small, contained
edit, and it makes every future recording replayable by unforged without
conversion.

## The change

Today the state hash is SHA-256 over the deterministic CBOR encoding of the
whole snapshot map (path to hex bytes). It is computed in two places:
`Snapshot::tree_hash` in `crates/coglet-fs/src/lib.rs` and
`Snapshot::tree_hash` in `crates/coglet-runtime/src/filesystem.rs`.

The new definition: the state hash is the git tree object id, in SHA-256 object
format, of the snapshot laid out as a directory tree.

- Each snapshot path becomes a tree path with the leading slash removed
  (`/data/count` becomes `data/count`).
- Every file is a blob with mode `100644`. Directories are trees with mode
  `40000`. No other modes exist.
- Entries are ordered by git's rule (byte order, with tree names compared as if
  followed by `/`).
- Object ids are 32 raw bytes inside tree objects and 64 lowercase hex
  characters everywhere else.
- The empty snapshot hashes to the SHA-256 empty tree id.

The ABI already forbids symlinks, hardlinks and device nodes. It does not yet
forbid `.git` names or a path that is both a file and a directory, and both
would break the one-to-one mapping (git refuses `.git` entries). The ADR adds
those two path rules, so every valid snapshot maps to exactly one tree.

[ADR-0031](../adr/0031-manage-port-and-filesystem-patches.md) §2 defines tree
identity as the `coglet-fs` CBOR hash. This change supersedes that clause, and
the ADR says so.

## What does not change

- `record_hash`, `code_hash`, `manifest_hash` and every other hash that is not
  a filesystem state hash keep their current definitions.
- The trace and journal formats keep their fields. Only the values of
  `initial_hash`, `final_hash`, `output_hash` and `state_hash` change meaning.
- No dependency on unforged, no git binary at runtime, no change to how code
  reaches a runtime (ADR-0031's `manage.patch` over plain files), no change to
  the adapter protocol.

## Later stages (not part of this change)

1. **Proof test.** Run the counter coglet for several steps with cogletd.
   Replay each step as an unforged job with caching off; the resulting tree ids
   must match the recorded state hashes. Repeat with caching on and confirm the
   jobs report cache hits.
2. **Hosted cold path.** Use unforged on Softmax-hosted runtimes for replays, forks and
   factory gates, where latency does not matter. Requires unforged to include
   the resolved worker image in its cache key and to emit receipts that say
   whether a job executed.
3. **Router on unforged.** Make the router's store an unforged repository, move
   code and state as SHA-256 refs, and use ref moves to implement install,
   start and deploy. Decide then whether steps become commits on devices.
4. **One effect path.** Replace the built-in `llm` capability with ordinary
   actions to an LLM adapter, record every adapter's outcomes for replay, and
   expose adapters as `/net` mounts. See [LLMs on unforged](llms.md) and
   [Secrets and adapters](secrets.md).

Each stage starts only after the previous one passes.

The secrets design in [Secrets and adapters](secrets.md) does not depend on
unforged: credentials live in adapters, coglets name them by id, and the bytes
stay out of every trace. Its rules can be adopted for today's adapters
whenever a spec for them is written, ahead of these stages.

## Instructions for the coglet swarm

Forward this section as-is.

**Goal.** Change the definition of the coglet filesystem state hash to the git
SHA-256 tree id, before more traces and fixtures are recorded against the old
definition. Scope is narrow: do not start any other unforged integration work.

**Process (ground rules apply).**

1. File an ADR first, per Ground rule 4. This is filed as ADR-0035
   (`docs/adr/0035-state-hash-git-tree-id.md`, PR #179), from the
   Decision, The change and What does not change sections of this document.
   Link it from every implementation PR.
2. Assign one card, owned by the Coglet Core session, titled
   `coglet: state hash = git SHA-256 tree id`. The orchestrator should treat it
   as blocking for any new golden fixtures and for the v0 freeze if the freeze
   has not closed.

**Implementation.**

3. Write one function that computes a git SHA-256 tree id from the ABI snapshot
   map, with no git binary and no new heavy dependencies (`sha2` is already
   used), so there is one implementation, not two. `coglet-runtime` depends on
   `coglet-fs` only with its `native-engine` feature, so either make that
   dependency unconditional for this function or put it in a crate both always
   use.
4. Replace both `tree_hash` bodies with that function. Keep the method names
   and each method's return type (`String` in `coglet-fs`, `Result<String>` in
   `coglet-runtime`); the id is 64 lowercase hex.
5. Update the comment in `crates/coglet-fs/src/git.rs` that says git object ids
   are not ABI tree hashes; the logs-only `GitStore` itself is unchanged.
6. Update `spec/abi.md` (Batching and filesystem, Journal and trace) and
   `spec/conventions.md` to state the new definition, and the CDDL / JSON Schema
   descriptions if they describe the hash.
7. Regenerate every golden fixture and recorded trace whose state hashes change,
   in `spec/fixtures/` and the crates' test data. Regeneration must come from
   re-running, not hand-editing.

**Done when (named tests).**

- `state_hash_matches_git_sha256_tree`: for a set of snapshots (empty, one file,
  nested directories, names that sort differently as trees versus files, binary
  contents), the computed id equals the id that real `git` produces in a
  repository initialized with `--object-format=sha256` (via `git mktree` or
  `git write-tree`). This test may use the git binary; production code may not.
- `state_hash_rejects_unmappable_paths`: a snapshot with a `.git` name, or
  with a path that is both a file and a directory, is refused before hashing.
- `state_hash_single_implementation`: `coglet-fs` and `coglet-runtime` return
  the same id for the same snapshot.
- The existing `cli_build_run_replay_equal_hash_list_and_inspect` and all
  conformance suites pass with regenerated fixtures, on both WAMR and Wasmtime
  where applicable.
- Green CI and an independent review, then merge through the queue.

**Out of scope.** Do not change the trace or journal structure, how code
reaches a runtime, the router, step semantics, or effect staging. Do not add an unforged
dependency. Questions about the later stages go to David, not into this card.
