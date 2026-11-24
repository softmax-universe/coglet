# Agent instructions

Read docs/plan.md first, especially the Ground rules section. Specs in spec/ are the source of truth, and spec changes go through docs/adr/. Port ideas, never code, from mindister or legacy/v0.

## CONTRIBUTING

- Follow the [plan's Ground rules](docs/plan.md#ground-rules-implementation-context-every-card-follows-these).
- Shared-file rule (Ground rule 7): only the skeleton card creates the root
  `Cargo.toml`, CI workflows, and `conformance/` layout. Later cards work in their
  own crate or directory; any necessary shared-file edit must be minimal and
  rebased on current main. Coordinate with the shared-file owner before editing.
- Spec changes require an ADR (Ground rule 4). Stop implementation, copy
  [the ADR template](docs/adr/0000-template.md) to `docs/adr/NNNN-short-title.md`,
  and submit and link the ADR PR before proceeding with the spec change.
- Merging goes through the merge queue on `main`. Once a PR has green CI and its
  independent review, run `gh pr merge <n>` (it enqueues; the queue sets the merge method). Do not
  update the branch from `main` to "catch up": the queue tests the merged
  result, so the reviewed head stays as it is. Rebase only for a real conflict.
  A PR that leaves the queue failed says why in its timeline; fix that and
  re-enqueue.
- A test that fails a required check and passes on rerun goes in `ci/quarantine.toml`
  with an owner card (docs/ci-quarantine.md). Repeated stress runs belong in the
  non-required `stability` workflow, never in a required job.
- Name tests in acceptance criteria. Open a PR with green CI, obtain independent
  review, then merge. A scaffold is not a protocol or conformance implementation.
- Disposable verification disk: `.cargo/config.toml` sets `profile.dev` /
  `profile.test` `debug = 0` and `incremental = false` while keeping
  `debug-assertions` and `overflow-checks` on. Do not change
  `[profile.release]` or `[profile.bench]`. Named check (must pass in both
  modes; `cargo test --release` uses release, `cargo bench` uses bench):
  `cargo test --locked -p coglet-types --test disposable_verify_profile` and
  `cargo test --locked -p coglet-types --release --test disposable_verify_profile`.
  Ordinary tests do not require ELF tools and allow intentional symbol builds
  (`CARGO_PROFILE_TEST_DEBUG=2` for tests; `CARGO_PROFILE_DEV_DEBUG=2` for builds).
  Before approving disposable Linux worker defaults, provision binutils (`readelf`)
  and run the separate artifact gate without symbol overrides:
  `cargo test --locked -p coglet-types --test disposable_verify_profile -- --ignored --exact compiling_profile_strips_dwarf`.
  This gate is Linux-only and deliberately ignored in ordinary portable runs;
  when explicitly selected, missing tools, invalid ELF output, or DWARF fail it.
  Do not interpret a zero-test run on another platform as artifact verification.
  After a disposable run, `cargo clean` unused private target dirs; never share
  writable target directories across concurrent owners.
