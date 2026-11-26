# Spike: coglet × unforged end-to-end proof

Question: can coglet's own runtime and unforged run the **real** `coglets/counter`
coglet as git objects, with both runners agreeing to the bit? This spike answers it
for Stages 1–4 of [Coglets on unforged](../../docs/cogent-design/unforged.md),
staged as in [the staging decision](../../docs/cogent-design/unforged-staging.md).
It is opt-in and adds **no** unforged dependency to the coglet workspace.

Everything here runs real code:

- the guest is `coglets/counter` compiled for `wasm32-wasip1` and packaged with
  `coglet_cli::build` (R3 canonicalization, provenance);
- each step is coglet's `Runtime::step` with a fresh WAMR `NativeEngine`
  (`coglet-runtime`, `coglet-engine-wamr`);
- state hashes come from `coglet-fs` / `coglet-runtime` (ADR-0035) and are
  checked against `unforged-gitobj` and the `git` binary;
- commits, receipts, resolvers, engine binding, chain replay, signatures,
  router, protected refs, read isolation, adapters and vault are unforged's own
  APIs (`unforged-gitobj`, `gecaos::coglet`, `gecaos::coglet_router`,
  `gecaos::coglet_protected_refs`, `gecaos::coglet_user_isolation`,
  `gecaos::coglet_adapters`) at the pinned commit.

## Run it

```sh
bash spikes/unforged-proof/run.sh                      # table on stdout, exit 0 = proof passed
bash spikes/unforged-proof/run.sh --json out.json --html out.html --verbose
```

Requirements: the Rust toolchain pinned by `rust-toolchain.toml` (the script adds
`wasm32-wasip1`), a C compiler, CMake (the WAMR build), `git` 2.45+ (reftable),
`ssh-keygen`, and access to `softmax-universe/unforged`.

`UNFORGED_DIR` selects the unforged checkout. When it is unset, `run.sh` looks for
`../unforged` next to this checkout (and next to the primary checkout when this is
a linked worktree). A checkout at the pinned commit is used directly; one that
merely contains the commit gets a temporary detached worktree, removed on exit.
Otherwise it clones the repository and checks the commit out. `UNFORGED_REV`
overrides the pin (default `1b5db9cc2c39059e58020fc2a1a254e6353c465a`).

All build output goes to `SPIKE_TARGET` (default `/tmp/coglet-spike-target`) with
`CARGO_PROFILE_DEV_DEBUG=0`; `--keep` keeps the proof's repositories under
`$SPIKE_TARGET/work` for inspection. Remove `SPIKE_TARGET` when done.

## What each act proves

All acts share one unforged repository (SHA-256 objects, reftable refs).
`code/manifest.json` carries the manifest inside the coglet tree, because ADR-0035
snapshots admit only `code/`, `data/` and `logs/`.

| Act | Proves |
|---|---|
| 0 | ADR-0035 parity. `coglet_fs::Snapshot::tree_hash`, `coglet_runtime::Snapshot::tree_hash`, `unforged_gitobj::snapshot_tree_hash_sha256` and `git write-tree` in a `--object-format=sha256` repository give the same 64-hex id for the empty snapshot, the counter's genesis and first three states, the `foo-bar` < `foo/` < `foo0` ordering case, and binary and deeply nested files. A `.git` component and a file/directory alias are refused by all four; symlinks, executables, gitlinks and empty subtrees are refused by `TreeProfile::CogletStrict`. |
| 1 | The [first test](../../docs/cogent-design/unforged.md#first-test). The hot path steps the real counter three times (1, 3, 2 `test.echo` events: `/data/count` = `1`, `4`, `6`, one `count=` log line each) plus a step whose manage write sets the count to `u64::MAX`, so the guest overflows: `Status: aborted`, tree equal to the previous one, `out/` empty. All objects go through one `MemoryObjectBatch`, with 0 `git` spawns by unforged's own counter. Each step is then replayed as an unforged `coglet-step` job (`execute_coglet_step` + `LocalDriverRunner`) whose resolver comes from unforged's stock `build_coglet_resolver_with_engine_bin`: `coglet-wamr-step`, this spike's helper running the same `Runtime::step` + WAMR, sits at `engines/coglet-engine` (`100755`, same bytes), `strategy` pins `engine = <label>@sha256:<blob oid>`, and unforged's own `drivers/coglet-step` execs it with no built-in fallback. With `CachePolicy::Bypass` all four `StepCommit` ids match and the receipts say `executed: true, cache: "none"`; a priming cache-on run writes `refs/notes/memoization`, and the next one matches with `executed: false, cache: "local"`. Bumping only the engine label misses the cache (`executed: true`, same trees). A `deterministic = false` resolver is never memoized. A resolver pinning a helper that exits 3 aborts the step, so the matches come from the helper running. |
| 2 | 100 hot-path steps of the real counter, then unforged's stock `replay_step_chain_in_store_with_engine` replays the whole chain with `ExternalCommandStepEngine::from_resolver(…, BatchStdio)`: the helper bytes come out of Act 1's resolver and one `coglet-wamr-step --batch-stdio` process serves all 100 steps. 100/100 commit ids match, 0 `git` spawns, receipts written as one `refs/notes/receipts` commit. An engine whose bytes differ from the resolver's pin is refused before it is spawned. As a control, unforged's own `replay_step_chain_in_store` replays 100 steps of its built-in interpreter module with 0 spawns. Ingest #2: two OpenSSH Ed25519 maintainers sign two SHA-1 commits over the identical counter policy tree; `import_sha1_commit_with_envelope` translates both (`Source-Commit` trailer, `refs/notes/source-commits`) and `verify_translated_commit_signature` resolves each maintainer and checks the SHA-1 ↔ SHA-256 tree bijection; a tampered tree fails it. |
| 3 | Router, on ADR-0040 `refs/namespaces/node-1/refs/users/counter/…`. A `hub-ref-statement` is encoded byte-identically by unforged and `coglet-types-hub0` (coglet's fixture vectors), signed with an iroh Ed25519 node key registered in `allowed_signers` with `refs=refs/namespaces/node-1/**`, and applied to a non-protected hub ref; the same legacy statement aimed at the protected `refs/users/<c>/state` is refused. The counter state is pushed as a `.promisor` pack without `data/` blob bytes (`git fsck --full` passes). [unforged#9](https://github.com/softmax-universe/unforged/issues/9)/[#10](https://github.com/softmax-universe/unforged/issues/10): `coglet-ref-statement/v1` statements bound to the hub's `unforged.repoId` move `refs/users/counter/state` (gen 1, 2) with watermark, operation receipt, target and `refs/notes/statements` in one transaction; a gen 1 retry after gen 2 returns the original outcome; changed-ticket, stale, wrong-repository, rerouted, cross-runtime, wrong-purpose and SHA-1 statements are refused; the factory cannot promote `published` and the runtime key cannot publish, while factory policy and kernel publish succeed; unsigned writes to the protected ref fail the shared gate. [unforged#11](https://github.com/softmax-universe/unforged/issues/11): the ref is hidden from node-2 (same coglet name, other runtime) and an ungranted auditor; their tip-OID wants (`blob:none`) and path reads are refused while the owner reads; a top-level grant does not reach the runtime-scoped owner; `logs.read` opens logs only; `data.read` plus the released blob reads `"6"` by path and by lazy promisor fetch; revocation refuses both at once; `refs/coglet-router/*` is hidden from non-admins. Compaction keeps the latest two step commit ids unchanged. |
| 4 | Adapters and vault. A catalog at `refs/adapters/github/catalog` projects `/net/github/create_issue/@xattr/{schema,effect}`; a `0x20 OP_STAGE_ACTION` call over the fd 3 framing yields `out/actions/42`, and dispatch is idempotent through `refs/notes/dispatched`. A policy patch asking for an ungranted capability stops with a `0x02` conflict tree and `PAUSED_CONFLICT`, state unmoved; after the grant and a deterministic `data/count` migration it merges cleanly. A `SecretRecord` in `refs/vault/policy` cannot be leased from a coglet step (`0x21 OP_LEASE_SECRET`), is leased by the authorized adapter with a value-free `refs/vault/audit` entry, is scrubbed from outcome bytes, and appears in 0 objects. |
| F | `git fsck --full` on the shared repository. |

## Known gaps

These are findings, not workarounds hidden in the harness:

- **Hot path.** unforged's `step_coglet_hot_inproc` still runs its built-in
  interpreter; the spike's hot path is its own in-process writer (coglet WAMR plus
  `MemoryObjectBatch`), standing in for `cogletd`. The cold path and chain replay
  run WAMR through stock unforged.
- **Single-step runtimes.** Each step runs a fresh coglet `Runtime` rooted at the
  prior tree (journal step 0); the unforged step number lives in the `StepCommit`
  trailer. Journal continuity across steps is not exercised.
- **Cache priming.** `CachePolicy::Bypass` neither reads nor writes memoization, so
  the cache-on pass is run twice: the first primes the cache.
- **hub0 schema.** `hub-ref-statement` in `coglet-types-hub0` admits only three refs
  and SHA-1 `new` ids, so the live SHA-256 statement is compared at the profile level;
  byte equality is shown on coglet's own fixtures.
- **Promisor fill.** unforged has no call that lets a device fill one promisor
  blob on the hub after a grant; Act 3 copies that one blob (oid checked) before
  the gated read. Serving it onward (`fetch_promisor_private_object`) is gated.
- **v1 ingress.** `coglet-ref-statement/v1` is applied by the hub library call
  `verify_and_apply_protected_ref_statement`; there is no wire endpoint for it
  yet, and Act 3 does not drive SSH/iroh transports (unforged's
  `tests/coglet_issues_9_10.rs` and `tests/coglet_issue_11.rs` do).
- **fd 3 transport.** Act 4 drives the fd 3 framing over an in-process socket pair,
  not a spawned child. The staged action blob is not byte-stable across runs, so the
  Act 4 state commit id varies while its merged tree does not.
- **Wall-clock ids.** Router notes, grants and checkpoint commits carry wall-clock
  times; their ids change between runs.
- **Commit ids depend on the helper build.** The resolver tree holds the
  `coglet-wamr-step` bytes and `engine =` names their SHA-256, and every
  `StepCommit` carries `Engine: <resolver commit>`. Rebuilding the helper with
  another toolchain or on another host changes the resolver, so step commit ids
  change while every tree id stays the same. Within one run, hot and cold ids must
  match exactly, and do.
- **Local only.** No CAOS worker runs here; `cache: "caos"`, worker images and
  `unforge stream exec --no-cache` (`?nocache=1` on CAOS) are covered by
  unforged's own tests and `softmax-universe/caos#502`.

## CI and dependency hygiene

The spike is outside the coglet workspace and has no tracked `Cargo.toml`,
`Cargo.lock` or `build.rs`: `Cargo.toml.in` is a template that `run.sh` fills in
under `SPIKE_TARGET`. So the dependency-mirror input key
(`scripts/ci/dependency_bundle.py` hashes every tracked `**/Cargo.toml`) is
unchanged, `dependency-capture` fetches only its fixed manifest list, and no CI job
ever tries to clone the private unforged repository. `Cargo.lock.in`, when present,
pins the resolved dependency set and makes the build `--locked`. The Rust sources
deny warnings and are `rustfmt --edition 2024` clean.

| Path | What |
|---|---|
| `run.sh` | Resolves unforged, builds the guest and harness, runs the proof. |
| `Cargo.toml.in` | Harness manifest template (`@SPIKE_DIR@`, `@COGLET_DIR@`, `@UNFORGED_DIR@`). |
| `src/counter.rs` | The shared step function: real counter under `Runtime::step` + WAMR. |
| `src/unf.rs` | Engine-bin resolver, hot path and cold replay over unforged APIs. |
| `src/act0.rs` … `src/act4.rs` | One file per act. |
| `src/bin/coglet_wamr_step.rs` | The engine bound in the resolver tree (`step-worktree <dir>` and `--batch-stdio`). |
| `src/main.rs` | Runs the acts, prints the table, writes `--json` / `--html`. |
