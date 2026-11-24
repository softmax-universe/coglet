# ADR-0026: Router push applied boundary

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet runtime receipt owner, following the Coglet orchestrator scope ruling
- Related PR: Runtime operation receipts, PR #134
- Supersedes / superseded by: None; clarifies ADR-0018 initial deploy ordering

## Context

[Hub 0.1](../../spec/hub-protocol.md#lifecycle-and-boundary-ordering)
permits an initial router push to stage objects for an authorized uninstalled
target, but forbids running code. Install consumes verified objects already in
local quarantine. Router push admission and terminal completion are distinct;
accepted-ref presence alone is not completion evidence.

[ADR-0018](0018-local-operator-control.md) requires the deploy saga to wait for
the original push child's applied receipt before dispatching install. Requiring
an installed runtime step to complete that initial push would create a cycle:
push waits for install while install waits for push.

## Decision

For an **uninstalled target**, the verified boundary is the authorized durable
commit of verified objects and the exact signed router ref into the quarantined
ref store consumed by install. Signature, delegation, artifact validation and
current authority must be checked before completion. The terminal receipt is
persisted for the **original push operation ID and original scope**, together
with explicit durable completion evidence. Merely finding an accepted-ref row
must never synthesize an applied receipt.

Admission first durably records a queued operation. A crash before completion
leaves that original ID queued. Recovery rechecks current authority and
reconciles that same intent to one terminal completion; it does not allocate a
replacement operation ID or repeat a confirmed effect. Effect confirmation and
terminal receipt commit atomically in their owning store. An identical retry
returns retained progress; conflicting intent fails with typed conflict.
Terminal receipts are immutable.

This boundary runs **no guest code** and creates no installed private namespace.
An applied pre-install push therefore leaves `hub.status.live_commit` null and
last committed runtime step at zero. Its receipt makes no applied-generation
claim. Install must consume the exact applied staged commit; a queued or failed
push is insufficient. Existing trusted local provisioning remains a distinct
spec-permitted source, not a bypass that relabels a queued remote push.

For an **installed target**, push completion instead occurs at the serialized
next runtime step boundary, after the current step finishes or aborts and before
next-step input/code pinning. This must preserve private `/data`, replace the
native engine safely, and retain original-ID completion across restart. That
implementation is owned separately by task `t_01a20ec3c6b6460b`, after the initial
deploy receipt implementation. Until it exists, installed-target pushes stay
queued honestly; pre-install completion must not be used to mark them applied.

There are **no new wire methods or schema fields**, and this ADR makes no frozen
`spec/` changes. Original actor, ticket, target and method bindings, fresh read
authority, indistinguishable unknown/cross-identity lookup denial, and the
absence of an applied-generation field remain unchanged.

## Alternatives

- Complete every push only after install: deadlocks the ADR-0018 initial saga.
- Report applied on accepted-ref lookup: confuses admission with confirmation,
  loses original-operation custody and can misattribute a later ref or actor.
- Report installed-target pushes applied at quarantine commit: falsely claims
  a runtime boundary and can change code mid-step.
- Introduce a remote activation method: unnecessary and violates the closed 0.1
  method set.

## Consequences

The initial deploy can reconcile push, then install, then start without a cycle.
Queued and terminal persistence must be separate observable durable states,
including across an actual daemon kill. No receipt read or retained retry renews
permission for effects. Expiry, revocation, failed verification or uncertain
storage must fail closed rather than manufacture completion.

This docs-only ADR supplies a decision, not implementation or qualification.
PR #134 must merge after or together with the accepted ADR, only after its own
independent exact-head review, required CI and applicable Apple guards. The
installed-target producer has separate acceptance; initial-deploy tests do not
qualify hot update, native replacement or private-data preservation.

## Verification

Implementation must exercise the actual device receiver and real `cogletd`,
not injected receipt objects. Required named tests include:

- `hub_receipt_original_scope_roundtrip` and
  `hub_receipt_denies_cross_identity`;
- `hub_receipt_survives_restart`: SIGKILL between durable admission and
  completion; same original queued ID after restart, then applied exactly once;
- `hub_receipt_queued_never_applied_without_boundary`;
- `hub_receipt_identical_retry_no_reexecute` and
  `hub_receipt_conflicting_intent_typed`;
- pre-install applied push with null live commit, install refusal for queued or
  failed staging, and installed-target push remaining queued;
- unchanged frozen `hub-receipt-*` fixtures against the production receiver,
  lifecycle regressions, mock and real WAMR where paths differ.

Receipt step must not exceed status step; separate reads do not claim atomicity.
Independent docs review and required CI qualify this ADR's text only.

## Approval

Pending independent exact-head review and merge through the main merge queue.
The orchestrator reserved ADR-0026 and authorized this initial-deploy scope;
that routing decision is not a substitute for PR review or implementation proof.
