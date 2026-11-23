# ADR-0009: Spec v0 freeze

- Status: Accepted (2026-11-23; freeze only, not implementation qualification)
- Date: 2026-11-23
- Authors: Coglet cross-spec freeze review
- Related PR: https://github.com/softmax-universe/coglet/pull/36
- Supersedes / superseded by: none

## Context

The v0 freeze is the serial gate in [the plan](../plan.md#phases-and-dependencies).
David directed merge-as-is and fix-forward on 2026-11-23. PR #36 merged as
`c9c22b6878898282848484c52df4d37c054f6720`; the published `spec-v0` tag stays on
that commit and MUST NOT be moved. The freeze owner accepts this ADR under that
instruction. Post-freeze corrections use reviewed follow-up PRs and contract
changes still require an ADR. This acceptance does not qualify implementations.

The proposal and milestone sections below retain their historical rationale;
references there to pending acceptance describe their pre-freeze state. The
Approval section records the final disposition, including the postmerge CI gap.

The orchestrator reserved 0009 for this freeze. ADR 0004 already describes the
hub protocol; the platform findings currently numbered 0002 will become 0008.
The conventions ADR keeps 0002. ADR 0005 is reserved for the separate policy
work and is not accepted by this freeze. ADR 0010 covers speech history/work.

## Decision (proposed)

Freeze the following normative documents and their associated schemas, golden
fixtures and language-neutral conformance case data at the merge commit tagged
`spec-v0`:

- `spec/conventions.md`
- `spec/abi.md` and `spec/manifest.schema.json`
- `spec/surface-protocol.md`
- `spec/capabilities/speech.md`
- `spec/capabilities/expression.md`
- `spec/hub-protocol.md`
- The common, ABI, surface, speech, expression and hub families under
  `spec/messages/`, as classified in `codegen/contracts.json`.

Policy, promotion and tickets remain drafts, outside this freeze; their gate is
M3. After the freeze, changes to these frozen contracts require a new ADR and
matching schemas, fixtures, conformance data and regenerated consumers. M1 can
motivate such an ADR; a freeze is not a claim that the contract is final.

### Engine decisions to record before acceptance

Accept ADR 0001's interpreter-first constraint and the platform findings (renamed
0008) as bounded evidence, not a claim of production-engine, EO1 or iOS
qualification. Preserve engine/build-specific compute accounting, signed-positive
FFI bounds, pre-instantiation memory admission and nondeterministic wall timeout
semantics. Resolve ABI allocation/handle cumulative budget accounting explicitly;
per-call reset must not silently give the guest a second full step allowance.

Amend ADR 0003 rather than accepting its current R4 unchanged: it currently
excludes the ABI's `coglet_v0` imports and the recorded non-filesystem effect
ledger. A conforming host function depends on the complete recorded step input
and host admission state, not only argument bytes and files. Its publish-time
transform remains a hub build-then-commit obligation; passing shape fixtures is
not qualification of that transform. Acceptance must distinguish the normative
policy from unperformed cross-architecture/fuzz qualification.

### Cross-spec repairs required before acceptance

The review identified the following joins which must be reconciled in this PR
with targeted negative controls, not waived as later runtime work:

1. Manifest `required_capabilities` versus operation names and entry names, and
   `exclusive_outputs` versus resource leases: the ABI's current subset rule
   cannot express the surface's separate operation/resource model.
2. Capability operation-to-payload bindings, including read arguments/results and
   action results, plus the defined status of illustrative fixture capabilities.
3. Guest delivery of action admission/rejection/outcome and the boundary between
   runtime-issued surface reads and the event-only guest ABI.
4. Deadline clock mapping must preserve a single durably recorded dispatch
   envelope for retries; a new clock sample cannot mutate an old action ID.
5. Hub relay must preserve closed capability payloads and private-event ownership;
   source sequencing must not interpret locally consumed runtime wakes as lost
   relayed messages.
6. Durable boundary writes while paused must remain auditable/exportable without
   inventing a guest step or hiding a trailing intervention.
7. Replay verification must check the hub's executable provenance requirement,
   not merely the Wasm blob's hash.
8. Path grammar must agree across filesystem/debug contracts. Prefer rejecting
   C0 controls and DEL in path components consistently (including embedded LF),
   with schema and semantic negatives; preserve content strings unchanged.
9. Behavioral case data must cover the declared speech operations (including
   backchannel and provider session/tool operations), not only their schemas.

The acceptance revision will record the exact resolution and test for each item.
See [the review ledger](../reviews/spec-v0-cross-spec.md) for current evidence and
items requiring further adjudication. This proposed ADR does not silently change
any existing wire schema.

### M-A amendment to ADR-0006 / PR #8: boundary export and terminal delivery

The step-only trace contract is amended before the v0 freeze. Preserve existing
pre-step `boundary` changes, but require explicit `initial_files`, `code_hash`,
`manifest_hash`, and `final_hash` in the trace header. Permit zero step records.
A nullable `trailing_boundary` is a separate hashed `boundary-record` with exact
output snapshot, before/output hashes, logical time, audit changes and `previous`
(last step hash or null). It records durable changes after the last step, including
an entirely standalone stopped/paused trace. It has no events, calls, usage or
step number and MUST NOT invoke an engine. Never hide durable changes pending a
future step. Initial writes preceding a first step remain in that step's boundary;
standalone exports use the boundary record. A continuation starts a new trace
from the preceding final snapshot and complete pending-work state. The archive
receipt authenticates the whole header, including the trailing record and ledgers.

Guest terminal notifications use only the durable inbox path. A rejection is a
`runtime.action_rejected` event with common `action-accept` payload restricted
to `accepted=false`; a completion is `action.outcome` with common `action-outcome`.
Both retain action ID/correlation and produce one ordinary `runtime.inbox` wake,
not a duplicate direct event. No `accepted=true` guest terminal receipt exists.
Runtime-generated rejection source is the target coglet; original surface outcome
source is retained, with runtime-generated outcomes permitted only under the
surface/hub authority rules. Runtime materialization uses boundary method
`runtime.receipt`, actor=target coglet, ticket=null; this is structural runtime
origin, not a hub RPC or permission granted by a payload string.

`initial_actions` contains complete committed actions still awaiting terminal
append, each present in lifetime `initial_effect_ids.actions`. New committed
emits enter the pending set; aborts do not. A terminal append consumes the pending
action atomically with inbox bytes and wake admission, not when the wake runs.
Duplicate/conflicting terminal delivery never appends twice, even with a new
source sequence or inbox ID; durable upstream retry receipts reconcile it.
Already-appended terminal messages are not pending actions. Replay validates
payload, identity/correlation, source binding and exact transition without live
dispatch. Finite exports may retain pending actions/wakes; no fake completion or
step drains them. This is a pre-freeze repair, not a backwards-compatible minor
version claim for the earlier 0.1 draft.

### M-B amendment: closed bindings and bounded qualification

This amendment resolves the remaining wire ambiguity without claiming runtime
qualification. The normative operation catalog in `spec/capabilities/bindings.json`
selects a schema rule for each direction, including closed empty read arguments
and closed empty successful action results. It does not widen a root union into
permission to use any sibling payload. `llm.complete` is an ABI/gateway operation,
not a surface action or an undeclared hub RPC; its input/output use the ABI
llm-request/llm-reply rules. Provider-specific input/output remain profile maps.
Runtime wake/receipt names are reserved to their producing authority; illustrative
fixture operations never become production capabilities merely by appearing in
an envelope or passing its grammar.

A guest emit deadline is interpreted in the runtime epoch at the ABI boundary.
Before first surface dispatch translate it once, then durably retain the complete
surface act envelope (including grant_epoch and translated deadline) before send.
Retries use those identical bytes even after new clock samples; reconnect must
reconcile the old request rather than translate/re-admit its ID. The original
ABI intent remains immutable in its journal. This clarifies the clock boundary,
not a new surface envelope or retry RPC.

One admitted step has ONE compute allowance and ONE absolute wall deadline,
covering initialization, abi_alloc and handle together. No second full allowance
may be installed for handle. A qualified WAMR mode must demonstrate exact
remaining-budget accounting across calls, positive signed-i32 FFI arguments and
fail-closed exhaustion before a further call. Reject unsupported/unqualified
modes before execution; the per-call spike setter does not prove this contract.
Separate host memory/stack bounds remain required qualification, not a promise
that linear pages bound total RSS.

The freeze inventory `docs/reviews/spec-v0-m1-gates.md` gives each remaining row
exactly one disposition. Real-engine metering, platform acceptance, transform
qualification, native speech behavior and authenticated relay authority are M1
qualification gates, NOT unspecified semantics or permission to run unqualified
code. Existing MUSTs still apply, including owner-only speech, per-target grants,
no synthesized success without authority, and no dispatch on an unqualified
engine. ADRs 0001/0008 are bounded spike evidence; ADR-0003 R1–R5 (with the R4
amendment) is the proposed publication policy, not a qualified implementation.
Their formal status transition and final freeze acceptance remain the freeze
owner's responsibility. No new ADR number or production acceptance is implied.

### M-C amendment: retry retention, clock domains and heard archive

This pre-freeze amendment supersedes the obsolete latest-batch-only paragraph
of the hub receipt section: retain EVERY admitted nonempty relay batch's digest,
offset and original next_offset for the device identity lifetime. A succeeds,
B succeeds, retry A returns A's original success, with no second routing.

Clarify ADR-0002's common deadline wording and extend the M-B boundary rule:
ABI guest intent is immutable runtime-epoch time; surface act is surface-epoch
time translated once before first dispatch. hub.actions is in the RECEIVING
runtime epoch. The sender must establish an authenticated/configured mapping
before sending and retain that mapped hub request; the receiver separately
retains its once-translated surface act. No new clock RPC is introduced: absent
mapping fails closed rather than interpreting a foreign clock numerically.
New samples/reconnects cannot change either retained envelope for an old ID.
These are contract vectors only; real crash/durable dispatch remains R-retry M1.

For heard history choose option (a), NOT the surface-retained narrowing (b).
Amend ADR-0006's ABI boundary-origin restriction and ADR-0010's unspecified private
recording: `runtime.heard` is one runtime-authored boundary-change at reserved
`/data/runtime/heard.cbor`, actor=target coglet, ticket=null. Its deterministic
CBOR `heard-state` records item/surface identity, duration, played position,
stopped state, aligned text spans and the checked derived heard prefix. The file
is guest-read-only, runtime-write-only, and not an inbox or synthetic guest event.
It fits ABI 86–104's closed applied-mutation record without new envelope fields;
its exact bytes participate in the existing snapshot and journal hash chain,
including initial and trailing/zero-step exports. Replay derives history solely
from exported journal state; no read response becomes a guest event and no live
surface/TTS/oracle can supply missing alignment. Private archive access follows
transcript privacy; live revocation does not rewrite historical audit bytes.

The ABI generator, rehashed positive/negative traces and actual harness timeline
regressions exercise these rules. No ADR status, engine/speech qualification or
existing M1 gate is accepted or changed by this amendment.

## Amendment M-D: heard-state construction from owner reads

The runtime constructs the private ABI `heard-state` conservatively from existing
owner-only `speech.heard {item, played_ms, text}` reads. This amendment introduces
NO wire fields or changes to position/barge_in. For each live item, duration_ms is
the maximum played_ms observed so far: known media extent at that recording,
never a predicted final duration. Positions never regress. A stopped item freezes
in full, including its spans; stopping may record one final read before the freeze.

Compare the read text's exact UTF-8 bytes with the concatenation of ALL archived
span text (not a normalized string or duration-ratio estimate). If played_ms is
greater than the last span end (zero for an empty list), and text strictly extends
that byte prefix, append ONE `{end_ms: played_ms, text: <new suffix>}` span.
Zero played_ms or empty text yields no span. Unchanged text, shrink or revision
also yields no span; played_ms/duration may still advance under stopped/no-regress
rules. Never replace existing spans. Derive heard solely from spans ending at or
before played_ms, with the ABI's existing UTF-8 cap. Thus revisions do not rewrite
history and unknown alignment never supplies guessed text.

Live collection is a separate boundary from offline replay. Only collection
consults authenticated owner reads; archive-only replay uses initial state and
hashed runtime.heard boundaries, with no live read or oracle injection. Tests
`test_owner_reads_wire_archive_fresh_replay` and
`test_owner_read_archive_mismatch_and_nonappend_rejected` exercise this join,
including a fresh replay process receiving only encoded archive bytes.

Richer native per-item alignment is NOT a v0 requirement. It is an explicit M1
qualification gate on surface Rust **t_01a205e673b63cb9** and surface Swift
**t_01a205e6f7d2d882**; it may only append spans consistently with these rules,
never revise an archived span. Core runtime/fs persistence, heard-path privacy
and debug exclusions, and hub.actions sender epoch-map retention have separate
named M1 owners/proofs in `docs/reviews/spec-v0-m1-gates.md`. No native or durable
implementation qualification, ADR acceptance or freeze acceptance is claimed.

## Alternatives

- Tag the independently tested individual specs immediately: rejected because
  component-level tests do not establish the contracts at their joins.
- Defer contradictory wire/manifest semantics to implementations: rejected;
  independent real and mock implementations would have no single contract.
- Treat missing real implementations as a spec-freeze failure: rejected. This
  gate requires executable case data and a tested harness, not a claim that the
  later implementation cards have already passed it.

## Consequences

No production conformance, physical-device performance, floating-point rewrite
qualification or cross-engine resource-cutoff equivalence is implied by this
freeze. Only loopback/self-test is currently registered in the harness. Preserve
that limitation in the freeze handoff. Future implementations must execute the
same applicable protocol suites without hiding skips.

## Verification

Required before acceptance:

- `python -m unittest discover -s spec/tests -v` in the pinned spec environment;
- `codegen/run --check`, generator tests and independent schema cross-check;
- `python -m unittest discover -s conformance/tests -v` and `conformance/run`;
- added cross-spec positive/negative regressions for the repairs above;
- CI green on the independently reviewed exact head and the landed main commit;
- unique numeric ADR filenames and a complete `docs/adr/README.md` index;
- remote `spec-v0` tag resolves to the verified freeze merge commit.

## Approval

Accepted by the freeze owner on David's merge-as-is / fix-forward instruction,
relayed by the orchestrator on 2026-11-23. The published tag records the original
freeze merge, not this status clarification. M-D's exact head
`910009f02e0c58affd9ea8761fda160e07d33f05` has green PR CI and separate-session
delta approval with 88 passing spec tests; its tree equals the freeze merge.
The final delta approval was recorded after merge, not retrospectively claimed
as a premerge gate.

The first postmerge conformance attempt (run `45153891816`) failed when rustfmt
exited 1 during codegen freshness; its diagnostic was hidden by stderr capture.
Attempt 2 succeeded on the identical tree, closing red main without a code fix;
the original failure's cause is unproven. The follow-up PR preserves formatter
stderr without weakening failure or freshness checks, so a future failure is
diagnosable. Required CI and independent review still gate that PR's merge.
No production protocol implementation was qualified by the harness loopback tests.

ADRs 0001/0008 remain bounded spike evidence and ADR-0003's amended R4 remains
the recorded-input policy, not proof of production transform qualification.
Their remaining formal-status reconciliation and the M1 inventory belong to
post-freeze owner `t_01a207e76001ac2c`; this freeze acceptance neither silently
accepts those implementations nor waives the ledger's fail-closed gates.
