# ADR-0019: M3 policy freeze and trusted-local promotion kernel

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet contributors
- Related PR: this PR
- Supersedes / superseded by: accepts the policy/promotion portion of ADR-0005 on acceptance; ticket scheduler implementation remains separate

## Context

M2's gridbot and cartpole now execute compiled guest Wasm through cogletd and
independent journal reexecution (PR83). M3 needs policy-enforced candidate
selection, rejection, probation and rollback. The draft contracts are in
[policy](../../spec/policy.md), [promotion](../../spec/promotion.md) and
[tickets](../../spec/tickets.md). The frozen hub protocol has no promotion or
ticket methods. Neither a successful gate nor a factory result grants deployment
authority. An accepted-ref store alone is not runtime activation.

## Decision

1. Freeze the policy and promotion contract at version `0.1` under annotated
   tag `policy-v0` only after this ADR is Accepted, implementation qualification,
   independent exact-head review and green merged CI. Change policy's required
   `status` literal from `DRAFT` to `FROZEN` in both schemas and fixtures; update
   document status, retaining the shape and all other normative rules. Old draft
   policy is rejected, not silently upgraded. The existing spec-v0 tag and hub
   method set remain unchanged. Promotion's signed-ref requirement is retained:
   the outbox retains exact `hub-ref` intent, verified bundle and operation
   context for existing `hub.git.push`; dispatch signs and durably retains the
   resulting statement after revalidation, not an unsigned abstract
   activate message. A configured adapter incapable of building that command
   makes promote admission fail `unavailable` without changing live state.
   Tickets stay draft; their existing candidate and terminal-result shapes are
   consumed without inventing provider fields. On acceptance, reconcile ADR0005
   and the ADR index. Keep `draft-validated` generator classifications until their
   syntax is compiler-supported: a freeze is not generated-code qualification.
   Add executable shared conformance cases for the local promotion boundary and
   run the same cases against real kernel and test adapter, labelled local-only.
2. Implement the trusted-local policy/promotion state machine in `coglet-hub`,
   alongside its existing registry and audit, not in a second registry/database.
   It is fixed kernel code, not guest-editable code or a factory plugin.
   `coglet-gate` owns sandbox evaluation only, as assigned in the plan. Add policy
   revisions, request receipts, proposals, generation, probation and boundary
   outbox tables to the existing hub Store. Its SQLite WAL/FULL transactions
   serialize current registration/policy checks and commit receipt, transition,
   audit and outbox together. Authority-owned revocation is additionally checked
   under the existing kernel boundary fence, not assumed to share the hub SQL
   transaction. Audit failure rolls back admission. No network endpoint is added.
   The kernel requires an explicit local runtime adapter; absence is refusal,
   never implicit installation or an authorized deployment claim.
3. Typed local authority interfaces separately authenticate actor/method/target,
   built candidate, gate evidence, human approval, capability/state compatibility
   and runtime boundary receipts. Strings/booleans provided by a factory are not
   authority. Verification must occur at propose and promote admission and again
   at boundary dispatch. The operator pins implementations and trusted identities;
   tests label fixture authorities as such. No permissive default implementation.
   Reinstallation checks enrolled control role and invalidates unactivated work;
   policy revision identifiers are never reused with different bytes.
4. Gate execution is a separate sandbox service. It evaluates exact rewritten
   `bin/step.wasm` bytes, not the source or original pre-rewrite module. Suites pin
   seed, world configuration, adapter/build identity and assertions. Journal replay
   uses recorded inputs and independent guest reexecution. Headless worlds use
   fresh Python-SDK gridbot/cartpole surfaces and the actual native daemon. Evidence
   binds full candidate/base IDs, executable digest, policy revision, suite, gate
   kind, trusted observation clock and immutable report artifact. Failed executions
   produce rejected evidence; a pass from another candidate is never transferable.
   No sandbox output reaches a live world. Existing M2 fixture provenance is not
   production build signing/qualification; production verification remains closed
   until a real qualified build authority and runtime adapter are installed.
5. Promotion admission produces a durable boundary command, not activation. The
   command binds coglet, proposal, old/new commit, monotonic generation, policy
   revision, ref intent/bundle (signed statement retained at dispatch), and exact
   actor/ticket operation context. The runtime adapter recovers the same
   `hub.git.push` operation through `hub.receipt`, cross-checks current
   `hub.status.live_commit`, and independently attests the applied signed
   generation through the qualified local boundary API described below; an
   arbitrary opaque receipt reference cannot complete activation. Late outcomes
   retain original operation context and never borrow another actor's receipt. Only a verified matching runtime receipt changes live state and starts
   probation. Duplicate/late/superseded receipts cannot activate code. Rollback
   cancels/fences a pending activation before queuing a higher-generation restore.
   It targets only retained stable code, never a caller-selected commit. Missing
   or unsafe restoration produces an output-fenced stop with an audited receipt;
   code rollback does not restore private data or undo world effects, and never
   makes the failed candidate last-known-good. Database
   DDL/DML/data restoration from ADR0013 is not implemented here; a candidate that
   needs database/data rollback must fail the state-compatibility check rather than
   treating code restoration as transaction undo.
6. Use a durable supervisor clock identity and monotonic milliseconds including
   downtime; reject regression. Use the same OS boot-identity plus continuous
   clock algorithm as `RuntimeService::supervised_now`/`ClockAnchor` (BOOTTIME on
   Linux, mach continuous time plus boot-session UUID on Apple), retaining a
   hub-owned anchor in the hub Store, not a transport-local `Instant`. Gate times
   are hub observation milliseconds. Runtime health/activation times require an
   authenticated explicit mapping; the runtime's logical time is not that clock.
   A changed/unverifiable epoch during probation or
   activation cannot complete probation: reconcile the same boundary command or
   fence/restore, recording uncertainty. No wall-clock fallback. Threshold equality
   and health timeout trigger before probation completion, including at expiry.
   Policy replacement never shortens an active probation. Samples bind current
   generation/commit, and stale/future samples do not reset health.
7. Request deduplication is enrollment-lifetime `(actor,coglet,correlation_id)` over
   exact retained request bytes. Identical retries recover original receipts, while
   conflicting bytes fail without changing the receipt. Rejected attempts have
   audited reason codes without caller secrets. A status read adds an observed
   audit. Retirement retains tombstones. Generation overflow fails closed.
8. A factory ticket's done result is only candidate input. Promotion attaches its
   immutable ticket/revision/evidence reference to the audit trail, independently
   revalidates all policy checks, and never changes a failed factory result into
   done. A rejected promotion has its own rejection audit, not a forged successful
   ticket. Ticket scheduling, spend accounting and provider adapters remain with
   the factory lane; this component does not implement that scheduler.

## Adapter qualification and recovery obligations

These are additional acceptance requirements, not fields added to frozen hub 0.1:

- `hub.status`, `hub.health` and `hub.receipt` do **not** report an applied
  generation. Commit equality, even with receipt step <= status step, cannot
  establish generation in an A→B→A sequence. The configured trusted-local adapter
  must independently attest the applied signed-statement watermark, exact
  operation identity and active executable at a serialized runtime boundary.
  Receipt/status reads are necessary cross-checks, not an atomic snapshot or a
  substitute for that attestation. Without this producer API activation remains
  `unavailable`; PR85's accepted-ref store does not qualify it.
- The command's signed ref generation equals the kernel's reserved per-coglet
  fence value. Every reserved value is burned, including cancelled/unattempted
  commands and terminal stops; gaps are legal, reuse is not. The adapter retains
  staged ref tip separately from live commit. A restore's signed `old` is the
  authoritative staged tip after reconciliation, not an assumed live commit.
- Admission retains exact command intent and bundle. Signing occurs only at
  dispatch after current build/policy/actor/ticket checks; its exact bytes are
  durably retained before sending. No network I/O occurs while holding the
  kernel fence. Cancellation invalidates the local signing/dispatch capability;
  an already signed or transmitted command is never assumed revoked by a local
  tombstone. Reconcile its original receipt, cancel only if unapplied, and use a
  higher-generation restore or an acknowledged output fence. A cancel conflict
  means reconcile an already applied operation, not successful cancellation.
  Unknown effects keep outputs fenced. Policy reinstallation applies this same
  recovery path to dispatched `activating` work, not simple deletion.
- Frozen health/status has neither generation-bound failure counters nor all
  probation health/integrity inputs. A separately authenticated local producer
  must supply cumulative per-activation step/budget counters, current
  capabilities/integrity and ordered generation/commit-bound health observations.
  Missing producer authority means probation is `unavailable`, never healthy by
  default. A pure probation reducer or fixture adapter cannot qualify this API.
- Authority checks use the shared control/dispatch fence and fresh checks before
  commit and every transport poll. Independent Authority database writers still
  require shared-Authority receiving-side validation; a remembered epoch is not a
  revocation lease. Derive and retain a unique common-grammar hub operation ID
  for each exact actor/target/request/command, never reuse it across command kinds.
- Bound and rate-limit status access before its `observed` audit admission;
  reserve independent control capacity for rollback/stop. Audit failure cannot
  leave a known failed candidate authorized to emit outputs: synchronously revoke
  the local output lease, refuse further activation, and reconcile/dispatch the
  preconfigured emergency `hub.stop` path with its original operation context.
  Until an authoritative stop receipt arrives, report fenced/uncertain, not stopped.
  Persist recovery diagnostics when storage recovers; never fabricate a successful
  rollback audit when a write failed. This fence/stop producer is itself required
  adapter qualification, not a best-effort callback.

Before acceptance, update promotion's acknowledgment/recovery text to reference
these trusted-local obligations (network integration still needs a separate ADR),
policy status and both policy schemas/fixtures, ADR0005 and the ADR index. **The
freeze tag remains blocked until real activation, health and emergency-fence
producers and the named integration proofs qualify; an accepted store is not enough.**

## Alternatives

- Add ad-hoc hub promotion methods: violates the frozen operation set and bypasses
  negotiation and per-method grants. A future network integration needs its own ADR.
- Let the factory write live refs or certify its own gates: collapses the trust
  boundary and allows policy bypass.
- Freeze a passing test stub or plain candidate-hash comparison: does not evaluate
  executable behavior and cannot establish native execution or rollback.
- Treat Git publication as activation: loses the durable runtime receipt and
  cannot distinguish staged code from an applied boundary.
- Use wall time after restart: can erase probation and accept stale evidence.

## Consequences

The new library and sandbox executor are testable independently of remote Git
streaming. Their local conformance is not full hub-wire or production qualification.
Actual runtime activation must use a registered adapter to existing signed ref,
CAS and boundary APIs; a simulated receipt is never advertised as that proof.
No production startup flag, capability grant, or automatic deployment is enabled
by accepting this ADR. Private state, prompts and credentials do not enter audits.
The annotated freeze tag is immutable; later changes require an ADR and a new tag.

## Verification

Required named tests (unexecuted at proposal time):

- `gridbot_candidate_promotes_after_native_evaluation`: real sandbox execution and
  independent replay, policy-bound evidence, admission != boundary activation.
- `cartpole_candidate_rejects_failed_evaluation`: failing behavioral assertion,
  audited rejection, no live/ref movement.
- `rollback_restores_last_known_good_with_new_generation`: activated candidate,
  threshold-equality failure, higher-generation restore and matching receipt;
  restart and late-ack controls.
- `audit_and_receipts_survive_restart_atomically`: receipts, rejected records,
  ticket/evidence bindings, injected audit failure and concurrent CAS.
- `policy_clock_and_authority_fail_closed`: wrong role/revision, forged/stale/future
  evidence, approval revocation, capability/state incompatibility, generation
  overflow and changed supervisor epoch.
- `probation_checks_timeout_before_expiry`: timeout/threshold equality, stale
  generation samples, all counters and current healthy observation at completion.

Run the unchanged control schema/fixture validators and generator freshness
checks. Required hosted CI must actually execute native sandbox acceptance, not
skip it. Independent review checks final exact head and every claimed integration
boundary. Where production streaming/activation is unavailable, preserve that
qualification gap explicitly; do not publish policy-v0 or close M3 as delivered.

## Approval

Proposed: no freeze tag or deployment is authorized by this text. Record the
independent exact-head review and accepting merge here before tag publication.
