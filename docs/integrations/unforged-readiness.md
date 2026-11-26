# unforged readiness for coglet integration

Status: readiness note and evidence boundary, not an adoption decision. For
David and the cogent swarm. Part of the
[staged integration](../cogent-design/unforged-staging.md) of
[Coglets on unforged](../cogent-design/unforged.md).

This note records what `softmax-universe/unforged` now implements for the 16
requests behind that design, what `softmax-universe/caos#502` adds on the CAOS
side, and what `spikes/unforged-proof/` exercises with real coglet code. It
changes nothing in `spec/`, `conformance/` or `crates/coglet-*`, and adds no
unforged dependency to the root workspace. Each later stage still needs its own
ADR and starts only after the previous one passes
([ADR-0035](../adr/0035-state-hash-git-tree-id.md#consequences)).

## Evidence boundary

**Established on unforged `main` at `5c1b48aa`**
(`5c1b48aa52a697885812c4ae4d5d73ae6ea94ad1`), by local runs on macOS with
`/usr/bin/git` for the differential checks:

- `cargo test -p unforged-gitobj --no-default-features --locked`: 7 passed.
- `RUSTFLAGS="-D warnings" cargo test --workspace --locked`: 254 passed,
  including the 16 coglet tests named below and the existing unforged suites.
- unforged has no hosted CI workflow. Nothing here has run in coglet CI.

**Not established by those tests**

- **No coglet code runs in them.** unforged's Stage 1 and 2 tests step a counter
  module built in-repo by `gecaos::coglet::build_counter_coglet_wasm()`, on
  unforged's own minimal interpreter (`ParsedWasmModule`, reached through
  `compute_pure_coglet_step`). Its hot path, `step_coglet_hot_inproc`, is a
  stand-in for `cogletd`, not `cogletd`. `coglets/counter` on WAMR runs only in
  the [spike](#the-end-to-end-proof).
- **The engine pin is a label.** `build_coglet_resolver` writes
  `engine = <pin>` into `/run/resolver/strategy` and a comment into the driver
  script; the stock `drivers/coglet-step` execs `unforge coglet-step-worktree`,
  which runs the built-in interpreter whatever the pin says. Changing the pin
  changes the cache key (so a cache miss is real), but nothing binds the pin to
  an engine build. [Coglets on unforged](../cogent-design/unforged.md#a-step-is-a-merge)
  decision 5 needs that binding.
- **CAOS is unchanged until #502 lands.** Against today's `caos-server`, a
  `--no-cache` replay falls back to submitting a salted `ArgTree`
  (`replay-<pid>-<nonce>`), which executes but leaves one salted result per
  replay in Redis and `refs/caos/res/`.
- **No device numbers.** Nothing was measured on iOS or the EO1; the commit
  rate on voice remains an [open question](../cogent-design/unforged.md#open-questions).
- **Ref layout differs from ADR-0040.** unforged uses `refs/coglets/<id>/…`;
  [ADR-0040](../adr/0040-hubs-and-coglets-on-unforged.md) puts a coglet's refs
  under `refs/users/<coglet>/…`. Nothing maps one onto the other yet.

## Landed commits

All four are ancestors of `5c1b48aa`, listed in push order.

| Commit | Requests | Adds |
|---|---|---|
| `015b3086` | #0, #5 (codec) | `crates/unforged-gitobj`, re-exported from `gecaos` as `gecaos::gitobj` |
| `7ea77796` | #9–#12 | `src/coglet_router.rs` (`gecaos::coglet_router`) |
| `01e1da3b` | #13–#15 | `src/coglet_adapters.rs` (`gecaos::coglet_adapters`), `fd 3` opcodes in `src/ipc.rs` |
| `5c1b48aa` | #1–#8 | `src/coglet.rs` (`gecaos::coglet`), `unforge coglet …` CLI |

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
| 2 | `coglet-step` driver, `deterministic` flag | 1 | `5c1b48aa` | `build_coglet_resolver`, `execute_coglet_step` | `stage1_nondeterministic_resolver_never_caches_locally_and_refuses_caos` |
| 3 | Cache off for one job | 1 | `5c1b48aa` | `CachePolicy::Bypass`, `--no-cache` | `stage2_ingest1_worker_upgrade_vs_local_and_caos_cache_reuse_and_nocache_control` |
| 4 | Resolved worker image in the cache key | 1 | `5c1b48aa` | `CaosDriverRunner` memo key `v1.3` | `stage2_ingest1_…` (as above) |
| 5 | Strict coglet trees | 1 | `015b3086`, `5c1b48aa` | `TreeProfile::CogletStrict`, `validate_local_tree_coglet_strict` | `stage1_coglet_strict_tree_validation_rejects_forbidden_entries_everywhere` |
| 6 | Receipts | 2 | `5c1b48aa` | `StepReceipt`, `record_step_receipt` | `stage1_counter_hot_vs_cold_…` |
| 7 | Signatures across SHA-1 translation | 2 | `5c1b48aa` | `import_sha1_commit_with_envelope`, `verify_translated_commit_signature` | `stage2_ingest2_two_signers_same_tree_and_bridge_signature_isolation` |
| 8 | Long chains as one job | 2 | `5c1b48aa` | `replay_step_chain` | `stage2_100_step_chain_replay_zero_git_spawns_and_receipts` |
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
  `deterministic = true`. See the engine-pin caveat above.
- **#3.** `CachePolicy::{Default, Bypass}` on the driver request. CLI:
  `unforge coglet step [--no-cache] [--advance-ref] --id <coglet_id> --resolver <resolver_rev> <state_commit> <input_commit>`.
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
  `cache: "none"`). CLI:
  `unforge coglet replay --id <coglet_id> --resolver <resolver_oid> <state_0_commit> <tip_state_commit>`
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
unforged-gitobj = { git = "https://github.com/softmax-universe/unforged", rev = "5c1b48aa52a697885812c4ae4d5d73ae6ea94ad1", default-features = false }
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
coglet code against unforged `5c1b48aa` from outside the root workspace:

```sh
bash spikes/unforged-proof/run.sh
```

It uses `UNFORGED_DIR` if set, otherwise a checkout of `5c1b48aa` next to this
repository, and exits 0 only if every assertion and `git fsck --full` passes.
It needs access to the private unforged repository.

| Act | Stage | Checks |
|---|---|---|
| 0 | ADR-0035 | `coglet_fs::Snapshot::tree_hash`, `unforged_gitobj::snapshot_tree_hash_sha256` and `git --object-format=sha256` agree on the empty snapshot, `coglets/counter` states and sort edge cases, and both refuse `.git` paths |
| 1 | 1 | [First test](../cogent-design/unforged.md#first-test): `coglet_counter.wasm` on WAMR for three wakes and one aborted step; replay with the cache off gives the same four commit ids and `executed: true`; cache on gives `executed: false`; a new engine pin gives `executed: true`; a non-deterministic resolver is never cached |
| 2 | 2 | A 100-step chain replayed as one job with no `git` spawn per step; two maintainers' SHA-1 signatures over the same tree both survive translation |
| 3 | 3 | A signed ref statement with an iroh key under per-ref rights; a state push without `/data/count`; fetch refused without a `data.read` grant and allowed with one; compaction keeps recent ids |
| 4 | 4 | A staged `/net/github` call through `0x20`; an ungranted patch pauses with `state` unmoved; a lease refused to a step and granted to an adapter, with no secret bytes in `git rev-list --objects --all` |

What the spike establishes: the codec agrees with coglet-fs, and coglet's
real engine produces the commit ids unforged expects. What it does not:
anything about `cogletd` itself, iOS or EO1 behaviour, CAOS (the spike runs
local jobs), or the spec. The [spike README](../../spikes/unforged-proof/README.md)
is the authority for exactly how each Act binds the engine.

## Open gates

Before any of this reaches `crates/coglet-*` or `spec/`:

1. **A Stage 1 ADR** (Ground rule 4, [plan](../plan.md#ground-rules-implementation-context-every-card-follows-these)):
   whether coglet depends on `unforged-gitobj`, how, and with which
   distribution (see [Consuming](#consuming-unforged-gitobj)).
2. **An engine binding in unforged.** The resolver's engine pin must name the
   engine build the cold path runs (WAMR at a given id), in stock unforged, not
   only in the spike.
3. **CAOS.** Review and deploy of `softmax-universe/caos#502`; until then
   replays on CAOS use the salt fallback. CAOS on SHA-256 remains a separate,
   larger track.
4. **Ref layout.** Reconcile `refs/coglets/<id>/…` with ADR-0040's
   `refs/users/<coglet>/…` before Stage 3.
5. **Stages 3 and 4** each need their own ADR and the spec changes listed in
   [Spec changes](../cogent-design/unforged.md#spec-changes) (`hub-protocol`,
   `abi`, `surface-protocol`, a new `spec/secrets.md`).
6. **Device measurements** of the commit, flush and push clocks on the EO1.
