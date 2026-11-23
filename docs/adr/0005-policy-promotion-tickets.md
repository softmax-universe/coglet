# ADR-0005: Draft policy, promotion and generic tickets

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet contributors
- Related PR: this PR (linked in the task thread before spec edits)
- Supersedes / superseded by: none

## Context

The Hub control, Fast loop and Factory sections of the design require a fixed
kernel to gate self-edits. The plan freezes policy at M3, not the v0 freeze.
Factory orchestration must remain replaceable by caos without provider fields.

## Decision

Add DRAFT 0.1 `spec/policy.md`, `policy.schema.json`, `promotion.md` and
`tickets.md`, with message schemas and executable fixtures. Import common
identity, correlation, error and CBOR definitions. Policy is per coglet and
revision-pinned, with replay/headless gates, optional human approval, probation
and explicit rollback thresholds. Hub control coglets require human approval.
Promotion uses durable proposals and compare-and-swap live generations, verifies
signed commits and independently trusted gate evidence, and records activation
only after the step-boundary acknowledgment. Factory results are candidates,
never permission to promote. Tickets expose generic submit/update/answer/amend/
cancel/result operations with revision fencing, budgets and scoped grants.

## Alternatives

- Freeze policy with v0: premature before M3 operational evidence.
- Pass orchestration-provider IDs and worker profiles: binds the core to Hermes.
- Treat gate claims or factory completion as authorization: crosses the kernel
  trust boundary and allows stale evidence or races to deploy unreviewed code.

## Consequences

No kernel or provider adapter is implemented here. Draft schema validation and
scripted state checks are not black-box implementation conformance. Later hub
and gate implementations must enforce authentication, signature trust, durable
idempotency and crash recovery. Deployment details stay in product configuration.
Draft evolution still follows common negotiated version compatibility rules.

## Verification

`python3 -m unittest discover -s spec/tests -v` validates positive/negative policy,
promotion and ticket fixtures, golden deterministic CBOR, and local cross-field
invariants. Both actual CDDL and JSON Schema validators check every fixture's
expected shape; semantic negatives pass both shapes and fail a separate local
invariant check. Array uniqueness for policy gate kinds and ticket edit paths
belongs to that semantic layer (standard CDDL has no generic unique-items
operator); the normative rejection requirement is unchanged. Required ticket
gate kinds are enumerable and remain a shape constraint in both schemas.
CDDL imports common rules by concatenation; positive uint64 refinements use an
explicit 1..uint64-max range because the pinned validator mishandles comparison
controls on range aliases. Closed array maps and forbidden sibling-alternative
keys use explicit empty types to avoid pinned-validator closure bugs without
changing the standard CDDL meaning. Lexical newline regressions, positive controls
and nested-map mutation tests exercise these paths.
Existing conformance CI discovers these tests without workflow edits.

## Approval

Independent review and acceptance recorded on this PR; this proposal alone does
not freeze the drafts or authorize deployment.
