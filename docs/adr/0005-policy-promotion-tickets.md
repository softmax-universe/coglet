# ADR-0005: Draft policy, promotion and generic tickets

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet contributors
- Related PR: this PR; supersedes reference PR #19
- Supersedes / superseded by: none

## Context

The design's fixed kernel owns promotion, probation and rollback. The factory
produces candidates through a provider-neutral ticket API. Policy freezes at M3,
not at spec-v0. PR #19 predates the frozen interfaces and shared reference validator.

## Decision

Add DRAFT 0.1 per-coglet policy, promotion and ticket contracts, importing frozen
common definitions without changing them. Operator-installed policy is revision
pinned; replay gates are mandatory; hub control roles require human approval.
Only the kernel may promote, after independently authenticating candidate build,
gate evidence, policy and approval. Admission is not step-boundary activation.
Probation and rollback are generation fenced and durable. Tickets bound editable
paths, debug authority, cumulative time/spend, revisions and cancellation custody.
Factory completion grants no deployment authority.

Use the existing exhaustive codegen registry: classify these three families as
`draft-validated`. Actual JSON Schema and the shared independent reference CDDL
validator check the handwritten fixtures. Wire/profile checks remain separate
from cross-field semantic checks and black-box implementation conformance.
Do not add validator changes or mutation generators in this card. The three
named array-type negatives are ordinary handwritten fixtures. Existing shared
validator tests remain responsible for their generic recursive audit.

No frozen spec, ALPN, hub frame or method enumeration is changed. These operation
payloads are a draft for Phase 3 integration, NOT currently legal frozen hub-wire
requests. A later integration ADR must assign negotiated framing/method grants,
publication bindings and activation receipt rules before network implementation.

## Alternatives

- Freeze at v0: premature before M3 evidence.
- Provider-specific ticket fields: couples the framework to one scheduler.
- Rebase PR19's validator workarounds/harness: obsolete after PR32 and outside
  this fresh card's scope; use standard closed CDDL maps and the shared oracle.

## Consequences

No kernel, factory, signature verifier or scheduler is implemented here. Fixture
validation and scripted case-contract checks are not real implementation
conformance. Existing hub publication provenance, actor/ticket isolation, ref CAS,
local grants and step-boundary rules remain authoritative and unchanged. An
unqualified integration fails closed rather than extending the frozen protocol.

## Verification

`test_control.ControlFixtures` validates selected schemas, public roots, golden
CBOR, handwritten wire negatives and locally observable cross-field invariants.
Run existing `codegen/run --check`, spec tests and conformance self-tests. Exact
head CI and independent review are required before merge. M3 freeze and actual
kernel/factory black-box qualification remain outside this draft.

## Approval

Recorded on the PR after independent review; Proposed does not authorize deployment.
