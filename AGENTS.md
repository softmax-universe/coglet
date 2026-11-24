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
- Name tests in acceptance criteria. Open a PR with green CI, obtain independent
  review, then merge. A scaffold is not a protocol or conformance implementation.
- Disposable verification disk: `.cargo/config.toml` sets `profile.dev` /
  `profile.test` `debug = 0` and `incremental = false` while keeping
  `debug-assertions` and `overflow-checks` on. Do not change
  `[profile.release]` or `[profile.bench]`. Named check (must pass in both
  modes; `--release` uses bench defaults and must not be given assertions):
  `cargo test --locked -p coglet-types --test disposable_verify_profile` and
  `cargo test --locked -p coglet-types --release --test disposable_verify_profile`.
  After a disposable run, `cargo clean` unused private target dirs; never share
  writable target directories across concurrent owners.
