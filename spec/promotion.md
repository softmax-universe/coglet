# Kernel promotion interface — DRAFT 0.1

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things. The runtime's own dispatcher is called the *event router*.

**Status: DRAFT; policy/promotion freezes at M3, not v0.**
[ADR-0005](../docs/adr/0005-policy-promotion-tickets.md), [PR #40](https://github.com/softmax-universe/coglet/pull/40).
Imports: [common](conventions.md), [policy](policy.md).
`messages/promotion.schema.json` and `messages/promotion.cddl` define closed
message records and shared control definitions. Concatenate `common.cddl` after
`promotion.cddl`; select `promotion` for the message union or `promotion-<name>`
for a specific definition. Deterministic CBOR is normative, JSON is a debug view.
This defines draft operation payloads only. The names `promotion.propose`,
`promotion.promote`, `promotion.rollback`, `promotion.status` are reserved here
as logical operations, **not legal methods/frames of frozen router 0.1**. No second
ALPN or implicit extension is authorized. A later integration ADR must assign
negotiated framing, per-method grants, publication and activation-receipt bindings
before any network implementation. Existing router build provenance, ref CAS,
actor/ticket checks and boundary ordering remain authoritative. The fixed kernel,
never the factory, owns live refs and activation.

## Values and authorization

A `commit` is a lowercase full Git object id (40 or 64 hex); abbreviated refs,
branch names and mixed Git object formats within one coglet repo are forbidden.
`repo` is a nonempty immutable registry repository identifier, not an arbitrary
URL the kernel fetches. Gate `suite` and evidence/approval references identify
immutable authenticated artifacts resolvable by the kernel; references themselves
are not proofs. The enclosing authenticated router supplies actor identity and
method grants; callers cannot set audit actor or gate authority by a string.
Common correlation ids identify requests; a common-shaped opaque `proposal_id`
identifies one durable proposal. A uint64 `generation` is a kernel-owned per-coglet
fence, incremented for each activation or terminal fenced stop; never reused.
Overflow fails closed. Coglets are independent even on one adapter.

All mutation requests include `version: "0.1"`, `correlation_id`, `op`, and
`coglet`. Responses retain correlation id and op. `ok:false` carries only the
common `error` besides these fields. An `ok:true` response carries only `result`.
Status read failures use the same error shape. Conflicting ids/state use `conflict`;
invalid gate shape uses `invalid_argument`, unverifiable authorization uses
`permission_denied`, unavailable evidence uses `unavailable`. No failure moves
live state. No operation gains authority from its version or correlation id.

## Operations

### Propose

Request adds `candidate`, `base`, `expected_generation`, `policy_revision`,
`gates` (nonempty report array), and optional `approval_ref`.
Each report is `{kind, suite, candidate, base, policy_revision, checked_at_ms,
status, evidence_ref}`; status is `passed` or `failed`. `checked_at_ms` is kernel
observation time certified by the trusted gate service (clock mapping is a service
contract); an untrusted factory cannot manufacture it.

The kernel verifies enrollment, caller's propose grant, trusted signature of the
built candidate including `bin/step.wasm`, repo/object format, base equals live,
generation, active policy and [gate/approval rules](policy.md). First installation
uses `base:null`, generation zero and must pass gates from the declared initial
state. No live or last-known-good ref changes. Success returns
`{proposal_id, state:"proposed"}`. Candidate, base, policy, evidence digests and
approval binding are immutable. At most one proposed/activating proposal per
coglet; competing proposals fail `conflict`. Rejected and invalidated proposals
remain in audit history, not current candidate status.

### Promote

Request adds `proposal_id` and `expected_generation`.
The kernel checks promote authority independently of propose authority and
rechecks signatures, policy revision, gate freshness, approval and current
grants. It compare-and-swaps the live generation/base against the proposal.
Success admission returns `{proposal_id, state:"activating"}` and durably queues
one signed `refs/heads/hub` update for the runtime's next boundary. This reply is
**not** activation. During `activating`, another mutation conflicts except an
authorized rollback, which cancels/fences the pending activation first.

The runtime verifies the signature and applies code only between steps. The
acknowledgment binds proposal, candidate and new generation; the kernel then
records live candidate and starts probation. A lost reply is recovered with
status/same-id retry, never assumed success. Late or duplicate acknowledgments
cannot activate a superseded generation. Failure or uncertainty after publication
fences outputs until reconciled or restored; it cannot return to `proposed` and
blindly publish again. There is no direct caller-supplied `live_commit` operation.
The retained pre-promotion **stable** commit is last-known-good; during probation
no second promotion is admitted. Initial install may have no last-known-good.

### Rollback

Request adds `expected_generation` and nonempty `reason`; it has no arbitrary
commit target. The kernel checks rollback authority and restores only its retained
last-known-good, invalidates the pending proposal and fences the failed generation.
Admission returns `{state:"rolling_back"}`; status reports completion only after
boundary acknowledgment. The kernel also invokes the same path automatically on
[probation triggers](policy.md), with actor=kernel and the triggering health/counter
evidence. Admission is durable before any branch movement. If already at the
stable generation with no pending mutation, a new rollback conflicts; a duplicate
of an admitted request recovers its original receipt.

Restoration verifies trust and current grants; a failed/absent/unsafe target stops
the coglet, fences outputs, records `stopped`, and alerts the operator. A restored
stable commit returns to `stable`, does not recurse into probation for the failed
candidate and does not toggle last-known-good back to that candidate. Code
rollback never undoes world effects or private state mutations.

### Status

Request has only the common request fields. Success result is:

- `generation`, `live_commit` (null before first activation),
  `last_known_good` (nullable), `candidate` (nullable);
- `state`: `empty`, `proposed`, `activating`, `probation`, `stable`,
  `rolling_back`, or `stopped`;
- `proposal_id` (nullable), `policy_revision`;
- `probation`: null or `{started_at_ms, expires_at_ms, consecutive_failures,
  step_failures, budget_violations}`;
- `recent_health`: bounded array of `{generation, commit, observed_at_ms,
  healthy}` (at most 64; no private snapshots).

`proposed`/`activating`/`probation` require a candidate and proposal;
`probation` alone requires a probation record and live=candidate. Other states
have null probation. Stable requires live=last-known-good and null candidate/
proposal. Empty has null commits/candidate/proposal and generation zero. Stopped
may retain the failed candidate for diagnosis, but grants no output authority.
Rolling-back status retains the failed proposal until acknowledged/fenced stop.
Status is an atomic durable snapshot, not proof an unreachable runtime is healthy.

## Recovery, audit and bounded idempotency

The kernel atomically persists the admitted request, transition, audit entry and
outbox item before responding/dispatching. The durable deduplication key is
(authenticated actor, coglet, correlation_id); byte-identical retries recover the
original response, conflicting reuse returns `conflict`. Deduplication records and
proposal tombstones MUST survive restarts for the entire coglet enrollment
lifetime; retirement preserves audit and MUST reject old identities/request ids.
This is a storage obligation, not a claim of exactly-once external effects.
Mutation reconciliation after restart compares runtime activation identity, signed
ref, durable outbox and generation; uncertainty fences outputs. Runtime epoch or
local time reset never erases a probation or makes it pass.

Each audit record has `version`, unique `audit_id`, `correlation_id`, `coglet`,
`actor` (authenticated role id, or literal `kernel`), `operation`, `phase`,
`generation`, `observed_at_ms`, `proposal_id` (nullable), `before_commit`,
`after_commit` (nullable), `policy_revision`, `reason`, `evidence_refs`.
Operation is propose/promote/rollback/status; phase is `admitted`, `rejected`,
`activated`, `probation_passed`, `rollback_triggered`, `restored`, `stopped`,
or `observed`. Audit ids use the shared opaque-id grammar. Actor, time and refs
are kernel-generated. Evidence includes gate/approval/signature refs for propose,
boundary receipt for activate, health/counter snapshot for automatic rollback,
and boundary/stop receipt for restore. Rejection records reason/error evidence
without leaking caller secrets. Status leaves an `observed` access record, not a
state change. Audit storage failure blocks mutations, and loss of audit custody
during activation/probation is a fail-closed recovery condition.

## Fixtures and future black-box cases

`fixtures/promotion/cases.json` covers every request/response operation, gate and
audit records and malformed shapes. `regressions.json` tests absolute-end lexical
matching with valid controls, including every inherited commit location.
`test_control.py` validates every fixture against both actual CDDL and JSON Schema,
checks selected rules/public roots, and separately checks binding and
status invariants, not signatures or real runtime behavior. A future shared router
suite MUST exercise competing propose/promote, stale base/generation/policy,
forged/expired evidence, duplicate requests across restart, delayed boundary acks,
rollback during activation, all probation triggers and first-install failure.
