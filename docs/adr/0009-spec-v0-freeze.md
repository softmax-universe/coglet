# ADR-0009: Spec v0 freeze

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet cross-spec freeze review
- Related PR: https://github.com/softmax-universe/coglet/pull/36
- Supersedes / superseded by: none

## Context

The v0 freeze is the serial gate in [the plan](../plan.md#phases-and-dependencies).
This proposal is **not an announcement that the gate has passed**. The tag
`spec-v0` must not be created until the cross-spec repairs, independent review,
and exact-head fixture validation, generated-output check and harness self-test
are green and the freeze PR is merged.

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

Pending cross-spec repairs and independent exact-head review. Do not infer
acceptance from this proposal's presence on a branch or in a draft PR.
