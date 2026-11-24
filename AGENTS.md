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
  independent review, run `gh pr merge <n> --merge` (it enqueues). Do not
  update the branch from `main` to "catch up": the queue tests the merged
  result, so the reviewed head stays as it is. Rebase only for a real conflict.
  A PR that leaves the queue failed says why in its timeline; fix that and
  re-enqueue.
- Name tests in acceptance criteria. Open a PR with green CI, obtain independent
  review, then merge. A scaffold is not a protocol or conformance implementation.
