# ADR-0027: Revalidate and resume running coglets after daemon restart

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet runtime maintainers
- Related PR: [#144](https://github.com/softmax-universe/coglet/pull/144); owner card t_01a20f45e1464258
- Supersedes / superseded by: none

## Context

[Hub lifecycle and boundary ordering](../../spec/hub-protocol.md#lifecycle-and-boundary-ordering)
defines start, stop, pause and status, but does not specify daemon restart.
`running` distinguishes a scheduler-enabled coglet from stopped/paused operation;
it does not mean the guest consumes CPU continuously when no event is ready.
Today the device can restore durable running metadata without enabling its native
scheduler. Retrying an applied start returns its original receipt, so even that
retry need not repair execution. A retained receipt describes a historical effect,
not current scheduler readiness.

## Decision

Choose boot revalidation plus resume, not implicit stop on daemon restart.
Propose the following addition to the lifecycle section of the hub spec, landed
with this docs-only decision before implementation:

- Before exposing the device service after daemon restart, recover every locally
  configured installed coglet. A durable running coglet resumes native scheduling
  only after revalidating the installed signed artifact and recovered executable
  bytes, installed identity, current delegation and capability grants, and the
  local kill switch. Recovery uses the installed identity, never a newer staged
  ref, and must not create fresh authorization from a stored receipt.
- The authority for automatic resume is the still-valid original start's actor,
  ticket scope and pinned authority generations. Commit that recovery context
  atomically with the running transition, bound to its original operation ID and
  intent digest. Missing/mismatched context in an older WAL is not authority.
  Require exact equality between retained, locally configured and currently
  authorized grant/delegation generations; any rotation, revocation or rollback
  requires a fresh authorized start, never adoption of newer authority on boot.
- Resolve durably admitted boundary mutations in their existing receipt order
  before considering resume. A pending stop/pause prevents guest entry; if its
  outcome cannot be established, keep execution fenced. A request that was never
  durably admitted creates no recovery intent. Do not fabricate a queued mutation
  for an operation whose effect and terminal receipt commit synchronously.
- Recheck time-bounded authority and kill state immediately before enabling
  execution after expensive validation, using the device's trusted authority
  clock (the same source used by ordinary start, not recovered logical time).
  Unknown time, unavailable native engine, missing/changed artifacts,
  revoked/expired authority or latched/unknown kill state never enable execution.
  For a readable, writable WAL with established identity, durably set faulted
  with an error and expose no scheduler; this includes missing recovery context
  and a latched kill switch. A fresh, distinct authorized start may leave this
  recovery fault only after all normal artifact/authority/kill checks pass and
  replacement recovery context commits atomically. Historical receipt retries
  cannot clear it. If WAL/identity integrity or durable fault persistence cannot
  be established, fail daemon startup without device readiness instead.
- Stopped, paused and faulted coglets never implicitly resume. Resume recovers
  committed data, logical clock and queued timers/events under the existing ABI
  recovery rules; it does not replay an uncertain guest attempt or redispatch an
  already committed physical effect. Existing unresolved-attempt fences remain
  authoritative. Running means eligible for native execution when input is ready,
  not a guarantee that a timer/event has already been processed.
- Recovery is not a new hub.start operation. Preserve original operation IDs,
  intent bindings, terminal receipts and ordering. Retrying a historical start
  must not reverse a later stop or create a second guest step on its own.

No wire enum, message schema, capability or receipt shape changes. Boot readiness
must be withheld until the recovery decision is complete; a status read is not
an implicit privileged start.

## Alternatives

- **Explicit start after every restart:** could honestly expose stopped until a
  fresh start, but discards running intent across a routine daemon restart and
  changes the existing idempotent-start expectation. Not chosen.
- **Trust the WAL running bit or retry receipt:** leaves status dishonest or
  bypasses present artifact/authority/kill admission. Rejected.
- **Resume on the first status/start request:** makes recovery depend on a remote
  reader and risks borrowing its authority. Rejected in favor of boot admission.

## Consequences

Existing installations without durable start authority context become faulted
rather than silently adopting an operator scope. They require a fresh authorized
start, so an upgrade does not strand them behind unavailable daemon readiness.
Boot validation adds startup cost; daemon readiness now certifies that durable
running intent has an admitted scheduler or an explicit non-running fault. WAL
integrity/persistence failure still prevents whole-device startup. No compiler
runs on the device and no legacy runtime code is imported.

## Verification

Implementation qualification remains separate from accepting this decision:

- `running_coglet_resumes_execution_after_daemon_restart`: start a real Wasm guest,
  kill/restart the actual cogletd process, observe a newly committed guest step
  from an event/timer after restart without a new start; use bounded protocol or
  durable-state readiness, never sleeps as readiness.
- `restart_revalidation_refuses_tampered_artifact`: tamper the retained artifact
  and prove restart refuses native execution and never serves running status.
- `restart_revalidation_refuses_expired_or_changed_authority`: expiry,
  revocation and both directions of generation mismatch refuse guest entry.
- `restart_missing_context_faults_until_fresh_authorized_start`: include an older
  WAL and mismatched operation/intent context, then prove fresh-start recovery.
- `restart_kill_switch_refuses_resume`: latched and unreadable kill state refuse.
- `restart_pending_pause_prevents_guest_entry`: exercise an actual durable queued
  pause; include committed stop/paused controls and unresolved mutation fencing.
- `historical_start_retry_does_not_reverse_stop_after_restart`: preserve receipts.
- Retain current lifecycle receipt, ABI journal/attempt-fence and surface suites.
- Run the restart proof 20 consecutive times under CPU load with raw evidence on
  the owner card. Required CI runs the named tests once; repeated stress belongs
  outside required CI under the existing flake policy.

## Approval

Independent exact-head review and accepting merge-queue PR are recorded in the
publishing PR and owner card. Proposed status is not implementation qualification.
