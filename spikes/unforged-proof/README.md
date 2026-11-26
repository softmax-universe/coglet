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
- commits, receipts, resolvers, signatures, router, adapters and vault are
  unforged's own APIs (`unforged-gitobj`, `gecaos::coglet`, `gecaos::coglet_router`,
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
overrides the pin (default `5c1b48aa52a697885812c4ae4d5d73ae6ea94ad1`).

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
| 1 | The [first test](../../docs/cogent-design/unforged.md#first-test). The hot path steps the real counter three times (1, 3, 2 `test.echo` events: `/data/count` = `1`, `4`, `6`, one `count=` log line each) plus a step whose manage write sets the count to `u64::MAX`, so the guest overflows: `Status: aborted`, tree equal to the previous one, `out/` empty. All objects go through one `MemoryObjectBatch`, with 0 `git` spawns by unforged's own counter. Each step is then replayed as an unforged `coglet-step` job (`execute_coglet_step` + `LocalDriverRunner`) whose resolver pins `bin/coglet-wamr-step`, this spike's helper running the same `Runtime::step` + WAMR. With `CachePolicy::Bypass` all four `StepCommit` ids match and the receipts say `executed: true, cache: "none"`; a priming cache-on run writes `refs/notes/memoization`, and the next one matches with `executed: false, cache: "local"`. Bumping only the engine pin misses the cache (`executed: true`, same trees). A `deterministic = false` resolver is never memoized. A resolver pinning a helper that exits 3 aborts the step, so the matches come from the helper running. |
| 2 | 100 hot-path steps of the real counter, then one in-process job replays the whole chain: 100/100 commit ids match, 0 `git` spawns, receipts written as one `refs/notes/receipts` commit. As a control, unforged's own `replay_step_chain_in_store` replays 100 steps of its built-in interpreter module with 0 spawns. Ingest #2: two OpenSSH Ed25519 maintainers sign two SHA-1 commits over the identical counter policy tree; `import_sha1_commit_with_envelope` translates both (`Source-Commit` trailer, `refs/notes/source-commits`) and `verify_translated_commit_signature` resolves each maintainer and checks the SHA-1 ↔ SHA-256 tree bijection; a tampered tree fails it. |
| 3 | Router. A `hub-ref-statement` is encoded byte-identically by unforged and `coglet-types-hub0` (coglet's fixture vectors), signed with an iroh Ed25519 node key registered in `allowed_signers` with `refs=refs/namespaces/node-1/**`, and applied with a reftable CAS plus `refs/notes/statements`. Statements outside the namespace, replayed or conflicting generations, and unregistered keys are refused. The counter state is pushed into a router repository as a `.promisor` pack without `data/` blob bytes (`git fsck --full` passes); reading `data/count` is refused without a grant while `logs/events.jsonl` reads; after a signed `data.read` grant it reads `"6"`. Compaction keeps the latest two step commit ids unchanged. |
| 4 | Adapters and vault. A catalog at `refs/adapters/github/catalog` projects `/net/github/create_issue/@xattr/{schema,effect}`; a `0x20 OP_STAGE_ACTION` call over the fd 3 framing yields `out/actions/42`, and dispatch is idempotent through `refs/notes/dispatched`. A policy patch asking for an ungranted capability stops with a `0x02` conflict tree and `PAUSED_CONFLICT`, state unmoved; after the grant and a deterministic `data/count` migration it merges cleanly. A `SecretRecord` in `refs/vault/policy` cannot be leased from a coglet step (`0x21 OP_LEASE_SECRET`), is leased by the authorized adapter with a value-free `refs/vault/audit` entry, is scrubbed from outcome bytes, and appears in 0 objects. |
| F | `git fsck --full` on the shared repository. |

## Known gaps

These are findings, not workarounds hidden in the harness:

- **Engine hook.** unforged's `run_coglet_step_worktree_dir`, `step_coglet_hot_inproc`
  and `replay_step_chain` call its built-in interpreter (`compute_pure_coglet_step`),
  not WAMR; `engine = <pin>` is a label. Act 1 therefore ships its own resolver whose
  driver runs coglet's engine, and Act 2's 100-step WAMR replay is a spike-local loop
  over unforged's in-process primitives. `replay_step_chain` needs an engine hook.
- **Single-step runtimes.** Each step runs a fresh coglet `Runtime` rooted at the
  prior tree (journal step 0); the unforged step number lives in the `StepCommit`
  trailer. Journal continuity across steps is not exercised.
- **Cache priming.** `CachePolicy::Bypass` neither reads nor writes memoization, so
  the cache-on pass is run twice: the first primes the cache.
- **hub0 schema.** `hub-ref-statement` in `coglet-types-hub0` admits only three refs
  and SHA-1 `new` ids, so the live SHA-256 statement is compared at the profile level;
  byte equality is shown on coglet's own fixtures.
- **Promisor fill.** unforged has no call that fetches one promisor blob after a
  grant; Act 3 copies that one blob from the device before the gated read.
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
- **Local only.** No CAOS worker runs here; `cache: "caos"` and worker images are
  covered by unforged's own tests and `softmax-universe/caos#502`.

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
| `src/unf.rs` | Resolver, hot path and cold replay over unforged APIs. |
| `src/act0.rs` … `src/act4.rs` | One file per act. |
| `src/bin/coglet_wamr_step.rs` | The helper pinned in the resolver tree. |
| `src/main.rs` | Runs the acts, prints the table, writes `--json` / `--html`. |
