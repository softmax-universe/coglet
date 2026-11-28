# unforged readiness for coglet integration

Status: readiness note and evidence boundary, not an adoption decision. For
David and the cogent swarm. Part of the
[staged integration](../cogent-design/unforged-staging.md) of
[Coglets on unforged](../cogent-design/unforged.md).

This note records what `softmax-universe/unforged` now implements for the 16
requests behind that design and for the three Stage 3 qualification gates David
filed as unforged#9, #10 and #11, what `softmax-universe/caos#502` adds on the
CAOS side, and what `spikes/unforged-proof/` exercises with real coglet code. It
changes nothing in `spec/`, `conformance/` or `crates/coglet-*`, and adds no
unforged dependency to the root workspace. Each later stage still needs its own
ADR and starts only after the previous one passes
([ADR-0035](../adr/0035-state-hash-git-tree-id.md#consequences)).

## Evidence boundary

**Established on unforged `main` at `1b5db9cc`**
(`1b5db9cc2c39059e58020fc2a1a254e6353c465a`), by local runs on macOS with
`/usr/bin/git` for the differential checks:

- `cargo test -p unforged-gitobj --no-default-features --locked`: 7 passed.
- `RUSTFLAGS="-D warnings" cargo test --workspace --locked`: 320 passed, 0 failed with `--no-fail-fast` across lib, bins, doctests and 29 of 30 test targets on `f425e131` (this head minus the last `origin/main` merge). `scala_machine`'s `wave3_stamp_drain_world_cas_shadow_non_sam_and_ws_backpressure` failed once there and fails 2 of 3 times on untouched `origin/main` too (a JVM timing flake). `craft_demo_e2e` does not compile on upstream `origin/main` since `23e5847` and was left out. After merging `51fa3dd`, lib, bins and the coglet, autosync, stream and otterwiki suites (173 tests) passed on `1b5db9cc`,
  including the 16 coglet request tests named below, the 12 named tests of
  [unforged#9, #10 and #11](#stage-3-qualification-gates-unforged9-10-11) and the
  existing unforged suites.
- unforged has no hosted CI workflow. Nothing here has run in coglet CI, and
  unforged#9–#11 stay open until David's independent exact-head review.

**Not established by those tests**

- **No coglet code runs in them.** unforged's Stage 1 and 2 tests step a counter
  module built in-repo by `gecaos::coglet::build_counter_coglet_wasm()`, on
  unforged's own minimal interpreter (`ParsedWasmModule`, reached through
  `compute_pure_coglet_step`). Its hot path, `step_coglet_hot_inproc`, is a
  stand-in for `cogletd`, not `cogletd`. `coglets/counter` on WAMR runs only in
  the [spike](#the-end-to-end-proof).
- **The engine binding is proven with WAMR only in the spike.** Stock unforged
  now binds an engine build to a resolver:
  `build_coglet_resolver_with_engine_bin` stores the executable at
  `engines/coglet-engine` (`100755`), pins `engine = <label>@sha256:<blob oid>`
  and has its own `drivers/coglet-step` exec that engine with no fallback to
  the built-in interpreter; `replay_step_chain_in_store_with_engine` refuses an
  engine whose blob differs from the pin. That is what
  [Coglets on unforged](../cogent-design/unforged.md#a-step-is-a-merge)
  decision 5 needs. unforged's own tests bind its reference engine
  (`unforge coglet-engine`); `coglet-wamr-step` is bound only in the spike.
  The label-only `build_coglet_resolver` is unchanged (same resolver ids).
- **CAOS is unchanged until #502 lands.** Against today's `caos-server`, a
  `--no-cache` replay falls back to submitting a salted `ArgTree`
  (`replay-<pid>-<nonce>`), which executes but leaves one salted result per
  replay in Redis and `refs/caos/res/`.
- **No device numbers.** Nothing was measured on iOS or the EO1; the commit
  rate on voice remains an [open question](../cogent-design/unforged.md#open-questions).
- **ADR-0040 refs are supported, legacy refs remain.**
  [ADR-0040](../adr/0040-hubs-and-coglets-on-unforged.md)'s
  `refs/users/<coglet>/…` (top level or under `refs/namespaces/<runtime>/`) is a
  first-class `CogletRefLayout::Users`, protected by unforged#10 and isolated
  per user by unforged#11. `refs/coglets/<id>/…` stays as a legacy layout with
  its old read contract (only `data/**` needs a grant).
- **v1 statements have no remote ingress.** `coglet-ref-statement/v1` is
  applied by `verify_and_apply_protected_ref_statement` on the hub; there is
  no wire endpoint that submits one yet. Raw pushes, shell, `integrate`, IPC and
  autosync are refused on protected refs; `push_coglet_state_without_data` and
  `compact_coglet_history` remain hub-local library calls that write refs
  directly.
- **unforged#11 boundaries.** A `have` line can still confirm that a guessed
  object id exists (no content is served). VFS `/dev` reads and `/net` adapter
  mounts are not bound to a read principal. `OP_READ_OBJ` admits objects that
  no protected ref reaches, so a sandbox can read its own fresh writes.

## Landed commits

All are ancestors of `1b5db9cc`, listed in push order.

| Commit | Requests | Adds |
|---|---|---|
| `015b3086` | #0, #5 (codec) | `crates/unforged-gitobj`, re-exported from `gecaos` as `gecaos::gitobj` |
| `7ea77796` | #9–#12 | `src/coglet_router.rs` (`gecaos::coglet_router`) |
| `01e1da3b` | #13–#15 | `src/coglet_adapters.rs` (`gecaos::coglet_adapters`), `fd 3` opcodes in `src/ipc.rs` |
| `5c1b48aa` | #1–#8 | `src/coglet.rs` (`gecaos::coglet`), `unforge coglet …` CLI |
| `e8765ce7`, `6d6a20fd` | #2, #3, #8; ADR-0040 layout | `StepEngine`, `ExternalCommandStepEngine`, `build_coglet_resolver_with_engine_bin`, `unforge stream exec --no-cache`, `CogletRefLayout` |
| `c539f46e`, `3aeede25` | unforged#9, #10 | `src/coglet_protected_refs.rs` (`gecaos::coglet_protected_refs`) |
| `31874a69`, `825631dc` | unforged#11 | `src/coglet_user_isolation.rs` (`gecaos::coglet_user_isolation`) |

`softmax-universe/caos#502` (open, for Malcolm's review; head `a9778e33`) adds
`GET /run?req=<argTree>&nocache=1` and the `X-Caos-Cache: hit|miss` and
`X-Caos-Image: docker://<ref>@sha256:<digest>` response headers, without
changing the response body. unforged reads the headers when present and uses
the salt fallback above when they are not.

## Request map

| # | Request | Stage | Commit | Main API | Test |
|---|---|---|---|---|---|
| 0 | In-process SHA-256 object writer | Foundation | `015b3086` | `unforged_gitobj::{encode_tree, encode_commit, LooseObjectStore, MemoryObjectBatch}` | `git_differential` (5 tests) |
| 1 | Reproducible step commits | 1 | `5c1b48aa` | `StepCommitSpec`, `write_deterministic_step_commit_inproc` | `stage1_counter_hot_vs_cold_local_and_caos_replay_and_engine_pin_invalidation` |
| 2 | `coglet-step` driver, `deterministic` flag, engine binding | 1 | `5c1b48aa`, `e8765ce7` | `build_coglet_resolver`, `build_coglet_resolver_with_engine_bin`, `execute_coglet_step` | `stage1_nondeterministic_resolver_never_caches_locally_and_refuses_caos`, `stage1_engine_bin_resolver_executes_embedded_engine_on_cold_path` |
| 3 | Cache off for one job | 1 | `5c1b48aa`, `e8765ce7` | `CachePolicy::Bypass`, `unforge coglet step --no-cache`, `unforge stream exec --no-cache` | `stage2_ingest1_worker_upgrade_vs_local_and_caos_cache_reuse_and_nocache_control`, `stage1_stream_exec_no_cache_forces_cold_runs_and_never_reads_or_writes_memoization` |
| 4 | Resolved worker image in the cache key | 1 | `5c1b48aa` | `CaosDriverRunner` memo key `v1.3` | `stage2_ingest1_…` (as above) |
| 5 | Strict coglet trees | 1 | `015b3086`, `5c1b48aa` | `TreeProfile::CogletStrict`, `validate_local_tree_coglet_strict` | `stage1_coglet_strict_tree_validation_rejects_forbidden_entries_everywhere` |
| 6 | Receipts | 2 | `5c1b48aa` | `StepReceipt`, `record_step_receipt` | `stage1_counter_hot_vs_cold_…` |
| 7 | Signatures across SHA-1 translation | 2 | `5c1b48aa` | `import_sha1_commit_with_envelope`, `verify_translated_commit_signature` | `stage2_ingest2_two_signers_same_tree_and_bridge_signature_isolation` |
| 8 | Long chains as one job, any engine | 2 | `5c1b48aa`, `e8765ce7`, `6d6a20fd` | `replay_step_chain`, `replay_step_chain_in_store_with_engine`, `ExternalCommandStepEngine` | `stage2_100_step_chain_replay_zero_git_spawns_and_receipts`, `stage2_external_engine_batch_stdio_replays_100_steps_with_one_spawn_and_zero_git_spawns` |
| 9 | Per-ref write rights | 3 | `7ea77796` | `evaluate_ref_write_acl`, `upsert_allowed_signer_with_refs` | `stage3_req9_per_ref_allowed_signers_ref_permissions_ssh_keygen_and_receive_pack` |
| 10 | Router ref statements, iroh keys | 3 | `7ea77796` | `verify_and_apply_ref_statement` | `stage3_req10_iroh_ed25519_hub_ref_statement_golden_vectors_cas_and_notes` |
| 11 | Push without `data/`, grant-gated fetch | 3 | `7ea77796` | `push_coglet_state_without_data`, `authorize_upload_pack_wants` | `stage3_req11_partial_push_without_data_blobs_fsck_and_read_grant_fetch_gating` |
| 12 | Step-history compaction | 3 | `7ea77796` | `compact_coglet_history` | `stage3_req12_compact_coglet_history_preserves_recent_oids_and_prunes_old_blobs` |
| 13 | Adapters as `/net` mounts, staging opcode | 4 | `01e1da3b` | `CogletNetMount`, `0x20 OP_STAGE_ACTION`, `dispatch_staged_actions` | `stage4_req13_net_adapter_mounts_xattr_fd3_staging_and_dispatched_idempotency` |
| 14 | Patch merge rules | 4 | `01e1da3b` | `apply_coglet_patch` | `stage4_req14_patch_merge_rules_migrations_and_paused_conflict` |
| 15 | Vault leases instead of env vars | 4 | `01e1da3b` | `VaultLeaseService`, `0x21 OP_LEASE_SECRET` | `stage4_req15_vault_secret_leases_sandbox_refusal_zero_git_secrets_and_scrubbing` |

Tests #1–#8 live in `tests/coglet_stage1_proof.rs` and
`tests/coglet_stage2_replay.rs`, #9–#12 in `tests/coglet_stage3_router.rs`, and
#13–#15 in `tests/coglet_stage4_adapters.rs` (which also has
`stage4_approval_gates_child_fd3_subprocesses_and_deterministic_patch_ids`).
#0's differential tests are in `crates/unforged-gitobj/tests/git_differential.rs`.

### Foundation: the object codec (#0)

`unforged-gitobj` is `#![forbid(unsafe_code)]`, has its own SHA-256 and SHA-1,
and spawns nothing. It encodes and decodes blobs, trees and commits in both
object formats:

- `hash_blob`, `encode_object`, `decode_object` and their `_sha1` forms;
  typed ids `Oid256` and `Oid1` (`Oid256::EMPTY_TREE_HEX` is the SHA-256 empty
  tree).
- `encode_tree(entries, TreeProfile)`, with git's entry order
  (`git_tree_entry_cmp`: `foo-bar` < `foo/` < `foo0`) and mode `40000` for
  trees.
- `snapshot_tree_hash_sha256(files)`: the ADR-0035 state hash of
  `(path, bytes)` pairs, with the leading `/` stripped and `CogletStrict`
  enforced. `write_snapshot_tree` returns the same root id plus every object,
  children first.
- `CommitSpec` for any commit, and `StepCommitSpec` for a step commit (below).
- `MemoryObjectBatch` stages objects in memory; `LooseObjectStore` writes zlib
  loose objects under `$GIT_DIR/objects` with temp file, fsync and rename, and
  `MemoryObjectBatch::flush_to` empties a batch into it.

The differential tests compare every id against `git --object-format=sha256`
and `sha1` (`hash-object`, `mktree`, `commit-tree`, `cat-file`, `fsck`).

### Stage 1: the proof test (#1–#5)

- **#1.** `StepCommitSpec::to_commit_spec` builds the commit described in
  [A step is a merge](../cogent-design/unforged.md#a-step-is-a-merge): parents
  `[state n, input n]`, author and committer
  `<coglet_id> <coglet@unforged> <logical_time> +0000`, message `step <n>`,
  trailers `Coglet-Step`, `Engine`, `Status: committed | aborted`, no
  signature. Observed usage (`StepUsage { compute, wall_us, mem_bytes }`) goes
  to `refs/notes/usage`, never the commit.
- **#2.** `/run/resolver/strategy` takes `driver = coglet-step`,
  `deterministic = true | false` (default `false`), `validation = coglet` and
  `engine = <pin>`. `execute_coglet_step` reads and writes
  `refs/notes/memoization` and dispatches to CAOS only when
  `deterministic = true`. `build_coglet_resolver_with_engine_bin(os, label,
  engine_bytes, deterministic)` (CLI `unforge coglet build-resolver
  --engine-bin <path>`) stores the engine at `engines/coglet-engine`, pins
  `engine = <label>@sha256:<blob oid>`, and its `drivers/coglet-step` execs
  `$UNFORGED_RESOLVER_DIR/engines/coglet-engine step-worktree <wt>`; a missing
  engine fails the step instead of falling back.
- **#3.** `CachePolicy::{Default, Bypass}` on the driver request. CLI:
  `unforge coglet step [--no-cache] [--advance-ref] --id <coglet_id> --resolver <resolver_rev> <state_commit> <input_commit>`
  and `unforge stream exec --no-cache` (also `unforge --no-cache stream exec`),
  through `MergeStream::merge_program_with_cache_policy`. With bypass the local
  runner neither reads nor writes `refs/notes/memoization`, pre-merge drivers
  included, and `CaosDriverRunner` sends `?nocache=1` (salt fallback without
  #502).
- **#4.** `CaosDriverRunner` resolves the base through CAOS `GET /eval` and
  `GET /resolve-image` and hashes the `@sha256:` digest into a `v1.3` memo key,
  so a worker image upgrade misses the local cache.
- **#5.** `TreeProfile::CogletStrict` admits only `100644` blobs and non-empty
  `40000` trees, and refuses symlinks, `100755`, gitlinks, `.git` in any case,
  control characters, backslashes and file/directory collisions. It is checked
  on the state tree, the input tree and the produced tree.

### Stage 2: receipts, signatures, chains (#6–#8)

- **#6.** Every step writes a `StepReceipt { runner, worker_image, executed,
  cache, commit }` to `refs/notes/receipts`, keyed by the step commit. CAOS jobs
  fill `worker_image` and `cache` from the #502 headers when present. CLI:
  `unforge coglet receipt <step_commit_oid>`.
- **#7.** `import_sha1_commit_with_envelope` keeps the raw signed SHA-1 commit
  in `refs/notes/source-commits` and adds a `Source-Commit: sha1:<40-hex>`
  trailer, and no longer skips a new upstream commit whose tree is unchanged.
  `verify_translated_commit_signature` checks the original OpenSSH signature
  against `allowed_signers` and that the SHA-1 tree maps to the local SHA-256
  tree. A local bridge signature never stands in for the source signer. CLI:
  `unforge coglet verify-source <local_commit_oid>`. CAOS itself stays SHA-1.
- **#8.** `replay_step_chain` replays an N-step chain in one process over
  `LooseObjectStore` and `MemoryObjectBatch`, with no `git` spawn per step, and
  writes one receipt per step (`runner: "unforged"`, `executed: true`,
  `cache: "none"`). `replay_step_chain[_in_store]_with_engine` takes any
  `StepEngine`: the built-in interpreter (default), or
  `ExternalCommandStepEngine` over `--batch-stdio` (one process for the whole
  chain, length-framed `CGQ1`/`CGA1`), per-step directories, or the
  `step-worktree` driver contract. `ExternalCommandStepEngine::from_resolver`
  runs the engine embedded in the resolver; a replay fails closed on an
  unreadable resolver or an engine whose blob differs from the pin. CLI:
  `unforge coglet replay --id <coglet_id> --resolver <resolver_oid> [--engine-bin <p> | --engine-from-resolver] [--engine-mode batch|dirs|worktree] <state_0_commit> <tip_state_commit>`
  (it accepts `--no-cache` but always executes).

### Stage 3: the router (#9–#12)

- **#9.** Ref globs per signer, either as trailing `refs=` / `allow-refs=` /
  `deny-refs=` comments on `$GIT_COMMON_DIR/info/allowed_signers` (the first
  four columns stay valid for `ssh-keygen -Y verify`), or in
  `$GIT_COMMON_DIR/info/ref_permissions` (`mode router`, then
  `<principal|@group|*> allow|deny <glob>,…`). Enforced before
  `git-receive-pack` moves a ref, on the TCP daemon, SSH and statement paths.
- **#10.** `iroh_pubkey_to_openssh` and `openssh_to_iroh_pubkey` map node keys
  to `ssh-ed25519`. `RefStatement` encodes as core-deterministic CBOR matching
  the `coglet-types-hub0` golden vectors. `verify_and_apply_ref_statement`
  checks the signature, the signer's ref rights and a monotonic `generation`,
  then moves the ref (compare-and-swap) and records the statement in
  `refs/notes/statements` in one reftable transaction.
- **#11.** `push_coglet_state_without_data` writes the state commits, trees and
  non-`data/` blobs as a SHA-256 pack with a `.promisor` file, with
  `extensions.partialClone = coglet-private`, so `git fsck --full` passes
  without the `data/` bytes. `authorize_upload_pack_wants` and
  `read_coglet_state_path_gated` refuse `data/` blobs unless a signed
  `CogletGrantRecord` under `refs/grants/` gives the caller `data.read` or
  `manage.read`.
- **#12.** `compact_coglet_history(os, coglet, keep_recent_steps)` keeps the
  newest K step commits with their ids unchanged, writes
  `refs/coglets/<id>/checkpoints/<step>` (`Status: checkpoint`,
  `Compacted-Steps`), sets the shallow boundary at the oldest kept step and
  prunes older steps and input blobs.

The router paths still call the `git` binary (receive-pack, upload-pack,
fsck) and need git 2.45+ for reftable. They belong on hosted runtimes, not
devices.

### Stage 3 qualification gates (unforged#9, #10, #11)

David filed these against unforged `5dc0a44` as prerequisites, not approval to
start Stage 3 ([ADR-0042 design consumer](https://github.com/softmax-universe/coglet/pull/217)).
They are implemented on `1b5db9cc`; they stay open for his exact-head review.

- **[unforged#9](https://github.com/softmax-universe/unforged/issues/9), atomic
  generations and original receipts.** Every statement (v1 or legacy) goes
  through one engine that writes a deterministic-CBOR watermark
  (`refs/coglet-router/watermarks/<sha256(scope)>`) and an immutable operation
  receipt (`refs/coglet-router/ops/<sha256(op)>`) and moves them, the target
  and `refs/notes/statements` in one `update_refs_atomic` with a CAS on each.
  The best-effort `.tsv` sidecar is gone. Corrupt or missing state fails
  closed; a retry returns the original outcome even after later updates; the
  same operation with different bytes conflicts; a generation at or below the
  watermark is stale.
- **[unforged#10](https://github.com/softmax-universe/unforged/issues/10),
  bound protected-ref statements.** `coglet-ref-statement/v1`, preimage
  `unforged.protected-ref.v1\0` + CBOR, binds `unforged.repoId`, the runtime
  namespace, the exact physical ref, old/new (SHA-256 only), generation,
  purpose (`policy-propose`, `publish-promote`, `state-export`,
  `inputs-export`, checked against the ref shape) and the
  actor/target/ticket/operation intent. Only `@kernel` (or `@publish`) promotes
  `published`; `@factory` only proposes `policy`. Legacy hub0 statements keep
  their vectors but cannot move protected refs. Raw receive-pack (TCP, SSH,
  iroh), shell, `integrate`/`map-ref`, `OP_REF_OP`/`OP_WORKTREE_COMMIT` and
  autosync pulls share one gate on `refs/{users,coglets,published,coglet-router}/**`.
- **[unforged#11](https://github.com/softmax-universe/unforged/issues/11),
  per-user read isolation.** The owner is `(runtime, coglet)`. Once any
  `refs/users/` ref exists, a non-admin may read an object only if it is
  reachable from a ref aspect it may read (ownership or a current signed grant
  on the owner's own grant ref); knowing an id or `--filter=blob:none` is not a
  grant. Enforced on the advertisement (v0/v1/v2), `want`/`want-ref`, path
  reads, the lazy promisor fetch and `OP_READ_OBJ`; scan errors deny.
  `refs/coglet-router/**` and `refs/grants/**` are not public.

| Issue | Named test | File |
|---|---|---|
| unforged#9 | `signed_ref_crash_atomic_generation_receipt` | `tests/coglet_issues_9_10.rs` |
| unforged#9 | `signed_ref_aba_rejects_stale_generation` | `tests/coglet_issues_9_10.rs` |
| unforged#9 | `signed_ref_retry_after_later_update_returns_original` | `tests/coglet_issues_9_10.rs` |
| unforged#9 | `signed_ref_corrupt_watermark_fails_closed` | `tests/coglet_issues_9_10.rs` |
| unforged#10 | `protected_ref_signature_binds_repository_namespace_and_intent` | `tests/coglet_issues_9_10.rs` |
| unforged#10 | `protected_ref_raw_push_cannot_bypass_statement` | `tests/coglet_issues_9_10.rs` |
| unforged#10 | `protected_ref_user_cannot_publish` | `tests/coglet_issues_9_10.rs` |
| unforged#10 | `protected_ref_all_exposed_mutators_share_gate` | `tests/coglet_issues_9_10.rs` |
| unforged#11 | `user_namespace_read_isolation_over_real_transport` | `tests/coglet_issue_11.rs` |
| unforged#11 | `private_oid_want_requires_current_grant` | `tests/coglet_issue_11.rs` |
| unforged#11 | `promisor_fetch_rechecks_revocation` | `tests/coglet_issue_11.rs` |
| unforged#11 | `private_history_scan_error_fails_closed` | `tests/coglet_issue_11.rs` |

The same files add controls: concurrent different-ref applies sharing the notes
ref, notes pruning, disk-error injection, autosync
(`protected_ref_autosync_pull_cannot_move_protected_refs`) and
`router_bookkeeping_and_grant_refs_are_not_public`.

### Stage 4: adapters, patches, vault (#13–#15)

These follow [Secrets and adapters](../cogent-design/secrets.md).

- **#13.** `publish_adapter_catalog` writes `refs/adapters/<name>/catalog`;
  `publish_coglet_grants` writes `refs/grants/<coglet>`. `CogletNetMount`
  lists only granted tools under `/net/<adapter>/`, with
  `<tool>/@xattr/{schema,effect,approval,name}`. `fd 3` opcode
  `0x20 OP_STAGE_ACTION` (`adapter, tool, call_id, payload`) checks the grant
  and stages `out/actions/<call_id>` with no network call.
  `dispatch_staged_actions` sends committed actions and records them in
  `refs/notes/dispatched`, so none is sent twice.
- **#14.** `apply_coglet_patch` merges `refs/coglets/<id>/policy` into `state`:
  `code/**` theirs, `logs/**` ours, `manifest.cbor` admitted against the grants,
  `data/**` migrated by `code/migrations/migrate` when present. A `base`
  mismatch, an ungranted manifest need or a failed migration writes a `0x02`
  conflict tree (`base`, `ours`, `theirs`, `conflict.info`), records
  `PAUSED_CONFLICT` at `refs/coglets/<id>/paused` and leaves `state` where it
  was; `resolve_paused_coglet` clears it.
- **#15.** `SecretRecord` metadata lives in `refs/vault/policy`, with guards
  against secret bytes entering git; the bytes live in a `VaultSecretBackend`
  (`MemoryVaultBackend`, `PrivateDirVaultBackend`, `MacosKeychainVaultBackend`).
  `fd 3` opcode `0x21 OP_LEASE_SECRET` is honoured only on authenticated
  adapter sessions and refused to coglet step sandboxes. `VaultLeaseService`
  checks `may_lease` and state, appends a value-free record to
  `refs/vault/audit` and returns the value in memory (default TTL 15 minutes).
  `scrub_leased_secrets` removes leased values from outcomes before they reach
  the next input.

## Consuming `unforged-gitobj`

Only `unforged-gitobj` is a candidate dependency for coglet. `gecaos` (the rest
of unforged) pulls in iroh, russh and tokio and calls `git`; the hot path must
not.

| Feature set | Gives | Dependencies |
|---|---|---|
| `default-features = false` | Hashing, encode/decode, `TreeProfile`, `snapshot_tree_hash_sha256`, `write_snapshot_tree`, `MemoryObjectBatch` | none |
| default (`disk`) | adds `LooseObjectStore` and `MemoryObjectBatch::flush_to` | `flate2` |

The dependency-free set is enough to hash state on iOS and the EO1. A Stage 1
card that adopts it, after its ADR, would add:

```toml
[dependencies]
unforged-gitobj = { git = "https://github.com/softmax-universe/unforged", rev = "1b5db9cc2c39059e58020fc2a1a254e6353c465a", default-features = false }
```

and `features = ["disk"]` (or the default features) where a crate writes loose
objects. Pin a full `rev`, never a branch. The crate uses edition 2024, which
the coglet toolchain already builds.

`coglet_fs::state_hash::snapshot_tree_hash` takes absolute paths to hex
contents; `unforged_gitobj::snapshot_tree_hash_sha256` takes paths to bytes.
Both must return the same id for every valid snapshot; the spike's Act 0
checks this. ADR-0035 asks for one implementation, so adopting the crate means
replacing `crates/coglet-fs/src/state_hash.rs`, not running both.

Two things block a core-crate dependency today:

- **The repository is private.** CI, the dependency mirror's preparation jobs
  and outside contributors cannot fetch a private git dependency. Making the
  crate public, publishing it, or vendoring it is a decision for its ADR.
- **The mirror key moves.** `scripts/ci/dependency_bundle.py` hashes every
  tracked `Cargo.toml` and `Cargo.lock`, so the change misses the existing
  Cargo bundle. Normal runs fall back upstream; the registry-denied proof
  needs a new candidate, `dependency-candidate-<PR_HEAD_SHA>`, published by an
  operator as described in
  [Candidate proof before a mirror change merges](../ci-dependency-mirror.md#candidate-proof-before-a-mirror-change-merges).

## The end-to-end proof

[`spikes/unforged-proof/`](../../spikes/unforged-proof/README.md) runs real
coglet code against unforged `1b5db9cc` from outside the root workspace:

```sh
bash spikes/unforged-proof/run.sh
```

[Run it](../../spikes/unforged-proof/README.md#run-it) says how it finds an
unforged checkout at `1b5db9cc`; it needs access to the private repository. It
exits 0 only if every assertion and `git fsck --full` passes. The spike crate
ships as `Cargo.toml.in`, materialized in a temporary directory, so it adds no
tracked `Cargo.toml`, `Cargo.lock` or `build.rs` and does not move the
dependency mirror key
([CI and dependency hygiene](../../spikes/unforged-proof/README.md#ci-and-dependency-hygiene)).

**How it binds the engine.** Through stock unforged only:
`build_coglet_resolver_with_engine_bin` stores `coglet-wamr-step` (the spike's
helper, linking `coglet-runtime`'s WAMR engine) at `engines/coglet-engine` and
pins `engine = coglet-engine-wamr@<build>+coglet-wamr-step@sha256:<blob oid>`.
Cold-path steps run through `gecaos::coglet::execute_coglet_step` with
`LocalDriverRunner`, so unforged's own `drivers/coglet-step` execs the WAMR
helper. Act 2 replays through `replay_step_chain_in_store_with_engine` with
`ExternalCommandStepEngine::from_resolver(…, BatchStdio)`: the helper bytes
come out of the resolver and serve all 100 steps over `--batch-stdio`.
Hot-path steps write their step commits in process with `MemoryObjectBatch`
and `LooseObjectStore`, naming the same resolver commit in `Engine:`.

| Act | Stage | Checks |
|---|---|---|
| 0 | ADR-0035 | `coglet_fs::Snapshot::tree_hash`, `unforged_gitobj::snapshot_tree_hash_sha256` and `git --object-format=sha256` agree on the empty snapshot, `coglets/counter` states and sort edge cases, and both refuse `.git` paths |
| 1 | 1 | [First test](../cogent-design/unforged.md#first-test): `coglet_counter.wasm` on WAMR for three wakes and one aborted step; the stock engine-bin resolver carries the helper bytes; replay with the cache off gives the same four commit ids and `executed: true`; cache on gives `executed: false`; a new engine pin gives `executed: true`; a non-deterministic resolver is never cached |
| 2 | 2 | A 100-step WAMR chain replayed by stock `replay_step_chain_in_store_with_engine`: one `coglet-wamr-step --batch-stdio` process, no `git` spawn (counted with `gecaos::git::git_spawn_count`), 100/100 ids; an engine whose bytes differ from the pin is refused; a control run of `replay_step_chain_in_store` on unforged's built-in module; two maintainers' SHA-1 signatures over the same tree both survive translation |
| 3 | 3 | `hub-ref-statement` byte-equal to `coglet-types-hub0`'s encoding (legacy, on a non-protected ref); a state push without `/data/count`; `coglet-ref-statement/v1` moving `refs/namespaces/node-1/refs/users/counter/state` with watermark, receipt, retry, stale, reroute, wrong-repo, SHA-1 and role checks (unforged#9, #10); per-user read isolation on that ref: advertisement, OID wants, path reads, promisor fetch, grant scope and revocation (unforged#11); compaction keeps recent ids |
| 4 | 4 | A staged `/net/github` call through `0x20`, dispatched once; an ungranted patch pauses with `state` unmoved, then merges once granted; a lease refused to a step and granted to an adapter, with no secret bytes in any git object |

What the spike establishes: the codec agrees with coglet-fs, and coglet's real
engine, driven through unforged's cold path, reproduces the hot path's commit
ids. What it does not: anything about `cogletd` itself, iOS or EO1 behaviour,
CAOS, or the spec. Its [Known gaps](../../spikes/unforged-proof/README.md#known-gaps)
and [What each act proves](../../spikes/unforged-proof/README.md#what-each-act-proves)
are the authority on detail.

## Open gates

Before any of this reaches `crates/coglet-*` or `spec/`:

1. **A Stage 1 ADR** (Ground rule 4, [plan](../plan.md#ground-rules-implementation-context-every-card-follows-these)):
   whether coglet depends on `unforged-gitobj`, how, and with which
   distribution (see [Consuming](#consuming-unforged-gitobj)).
2. **Exact-head review of unforged#9, #10 and #11.** Implemented and green
   locally on `1b5db9cc` (see
   [Stage 3 qualification gates](#stage-3-qualification-gates-unforged9-10-11));
   David's independent review and green CI qualify them, not this note.
3. **CAOS.** Review and deploy of `softmax-universe/caos#502`; until then
   replays on CAOS use the salt fallback. CAOS on SHA-256 remains a separate,
   larger track.
4. **Stages 3 and 4** each need their own ADR and the spec changes listed in
   [Spec changes](../cogent-design/unforged.md#spec-changes) (`hub-protocol`,
   `abi`, `surface-protocol`, a new `spec/secrets.md`).
5. **Device measurements** of the commit, flush and push clocks on the EO1.
