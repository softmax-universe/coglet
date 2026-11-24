# ADR-0027: Revalidate and resume running coglets after daemon restart

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet runtime maintainers
- Related PR: linked by the publishing PR and owner card t_01a20f45e1464258
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
  ticket scope and pinned authority generations. Persist that recovery context
  with the running transition. Missing context in an older WAL is not authority:
  refuse automatic resume until a new authorized start establishes it.
- Recheck time-bounded authority and kill state immediately before enabling
  execution after expensive validation. Unknown time, unavailable native engine,
  missing/changed artifacts, revoked/expired authority or latched/unknown kill
  state never enable execution. If recovery cannot safely establish a usable
  service, daemon startup fails without exposing a running status. A recoverable
  per-coglet failure may instead be durably represented as faulted with a shared
  error; it must not be reported as running.
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

Existing installations without durable start authority context fail closed rather
than silently adopting an operator scope. They require a fresh authorized start.
Boot validation adds startup cost; daemon readiness now certifies that durable
running intent has an admitted scheduler, not merely readable metadata. Failure
may prevent whole-device startup rather than exposing partial readiness. No
compiler runs on the device and no legacy runtime code is imported.

## Verification

Implementation qualification remains separate from accepting this decision:

- `running_coglet_resumes_execution_after_daemon_restart`: start a real Wasm guest,
  kill/restart the actual cogletd process, observe a newly committed guest step
  from an event/timer after restart without a new start; use bounded protocol or
  durable-state readiness, never sleeps as readiness.
- `restart_revalidation_refuses_tampered_artifact`: tamper the retained artifact
  and prove restart refuses native execution and never serves running status.
- Cover expired/revoked delegation/grants, missing recovery context, kill state,
  stopped/paused recovery and historical start retry after a later stop. Retain
  the current lifecycle receipt, ABI journal/attempt-fence and surface suites.
- Run the restart proof 20 consecutive times under CPU load with raw evidence on
  the owner card. Required CI runs the named tests once; repeated stress belongs
  outside required CI under the existing flake policy.

## Approval

Independent exact-head review and accepting merge-queue PR are recorded in the
publishing PR and owner card. Proposed status is not implementation qualification.
