# ADR-0015: Executable companion fixtures for archive-profile replay

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet runtime lane
- Related PR: follows [PR #63](https://github.com/softmax-universe/coglet/pull/63); accepting PR recorded in review history
- Supersedes / superseded by: none; supplements ADR-0006 and ADR-0009

## Context

The 80 frozen boundary-receipts, provenance and heard-archive cases pin the
empty Wasm module. Archive consistency via `ObservedTransition` is not independent
guest replay (see [runtime qualification](../../crates/coglet-runtime/abi-qualification.md)
and [ABI journal contract](../../spec/abi.md)). The `spec-v0` bytes,
existing case names and tag must remain immutable. Runtime-only trailing and
zero-step boundary records are not guest invocations. Invalid archives must be
rejected before execution; their purported transitions are not runnable evidence.

The source inventory contains 32 guest records: 21 boundary-receipts, eight
provenance and three heard-archive, including records in invalid archives.
Only seven have nonempty host-call lists. Empty-call handles are still guest
execution obligations, not exclusions. The orchestrator reserved 0015 for this
ADR and confirmed that all 32 guest records require execution coverage.

## Decision

Add separately named executable companion case files under `conformance/abi`,
with a deterministic source-to-companion inventory. Preserve all frozen files.
Keep the archive validation operation and `engine_invocations=0` contract unchanged;
exercise independent execution in a separately named native replay test, not by
silently changing `abi.validate_trace` into an executing operation. No existing
wire message, ABI import/export, hashing rule or frozen schema is changed.

For each source guest record, pin an executable companion or a corresponding
valid executable baseline, including handles with an empty host-call list.
Pin real Wasm bytes plus guest source and a reproducible build recipe. New
archives recompute module/provenance, manifest, initial/output state, record and
chain hashes as necessary using the existing core-deterministic CBOR and tree
hash rules. They do not claim equality to frozen hashes whose `/code` contents
necessarily differ. Source case names and record positions map explicitly to
companion records. Keep expected outputs separate from the program's independently
supplied input and do not feed recorded calls, return values or output files to
an engine substitute. Guest source may encode scenario intent; it must execute
the actual WAMR imports/exports and host logic.

For invalid sources, retain archive rejection cases and map each guest record to
a valid executable baseline plus its deliberate corruption. Execute that baseline
and compare its per-step state hash; separately prove rejection of the corrupt
archive before guest entry. This covers the guest behavior represented by an
invalid record without claiming that an inconsistent transition can execute
successfully. Where corruption invents an impossible guest behavior, identify the
valid baseline behavior and the precise changed field explicitly. Baselines may
be shared, but no source guest-record position may be omitted merely because its
archive is invalid. A source record may have both baseline-execution and negative
validation obligations.

Maintain a complete machine-checked source inventory: every case and record is
classified as a guest step (with an executable mapping), a runtime-only boundary,
or an archive-level rejection obligation. Rejection obligations supplement rather
than replace executable mappings for the 32 guest records. Missing mappings,
unclassified records or an unexpected source inventory fail the test; none are
silent skips. Boundary-only records use runtime validation, not invented guest
invocations. Preserve the original negative-case assertions.

Replay accepted companions/baselines through a fresh WAMR execution context using
only pinned code, manifest, initial state and legitimate recorded external inputs.
Within a multi-step archive preserve the real committed state between handles.
Compare actual per-step state hashes with pinned companion expectations and
independently reconstructed state. Preserve checks for effects, status, host
results and chain integrity; trailing runtime boundaries use their existing
non-guest validation path. No network or physical device is required; external
replies remain recorded inputs, not fabricated engine results.

## Alternatives

- Rewrite frozen fixtures or move `spec-v0`: rejected; destroys the baseline.
- Supply recorded calls through `ObservedTransition`: useful archive validation,
  but rejected as independent guest execution evidence.
- Count corrupt traces as replay successes, or exclude their guest records:
  rejected; wrong admission semantics or incomplete execution coverage.
- Replace rather than supplement the existing ABI suite: rejected; loses negatives.

## Consequences

This is an additive conformance corpus, not a wire/ABI revision. Module changes
alter code-containing state hashes, so companion hashes are independently pinned.
Build provenance does not grant publication authority. Native guest execution
uses existing WAMR resource fences. Existing archive validator behavior remains
unchanged. Shared harness edits require coordination with its owner. Implementation
starts only after independent ADR review and merge; this PR adds no fixtures.

## Verification

Planned named acceptance tests (not yet implemented):

- `executable_archive_inventory_covers_every_source_record`: inventory all 80
  cases and their records, require executable mappings for all 32 guest records
  across the three profiles, and classify boundary/rejection obligations.
- `executable_archive_records_reexecute_on_wamr_with_identical_step_hashes`:
  execute every mapped guest step, including empty-call handles and valid
  baselines for invalid sources; compare each actual state hash with its pinned
  executable expectation, not the old empty-module hash.
- `executable_archive_corruptions_reject_before_guest_entry`: preserve negative
  assertions and prove zero guest entries for rejected archives.
- `executable_archive_replay_rejects_guest_program_mismatch`: substitute a
  different executable guest while preserving a structurally consistent archive;
  require execution mismatch, not just an earlier module-digest failure.
- `frozen_archive_corpus_and_spec_v0_remain_unchanged`: compare frozen files with
  the existing freeze and preserve tag `c348fdeca3c162bdaeef0f65b4da74f5ecfb59f3`.

Extend `crates/coglet-runtime/tools/check-abi.py` (the actual repository path) to
require fresh exact case inventories for additive companions, zero failures and
zero skips, while preserving all 128 frozen cases. Native replay must fail if
native-engine/WAMR is unavailable rather than reporting a successful skip. Require
exact-head CI green and independent execution/session review before landing.

## Approval

Pending independent review and acceptance by merging the ADR-only PR. The PR's
exact-head review records the independent verdict; merge records acceptance.
This proposed document is not an implementation waiver.
