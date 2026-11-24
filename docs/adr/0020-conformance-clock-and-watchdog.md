# ADR-0020: Separate surface fixture time from runner watchdogs

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet contributors
- Related PR: https://github.com/softmax-universe/coglet/pull/100
- Supersedes / superseded by: none

## Context

Surface protocol 0.1 requires durable grants and action receipts, exclusive action
and read deadlines, monotonic elapsed time, health, and an independent local kill.
It does not promise a 200 ms durable response on an arbitrarily scheduled runner.
The surface JSON cases currently use a 200 ms wall-clock receive budget while the
reference and mock fixture adapters advance protocol time with `Instant`.

PR87 separated startup readiness from the first receive. PR100 reduces the first
action's durable barriers from three to two without removing required durability.
Neither prevents a single epoch or rejection sync from exceeding 200 ms.
Main readiness run 45340588562 at e9e58170 failed read-config/action negotiation
(step 2), conflicting-id negotiation, and deadline-exclusive admission. Queue run
45341088504 at 0d0027af522b91660c5ae4e38102ad0827bfc5da failed read-config step 2
and kill-latched step 7. Both emitted readiness and had zero step-0 timeouts.
These logs establish the residual failure, not the unrecorded syscall duration.

Simply increasing receive limits also lets fixture time pass the fixed 1-second
request deadlines, 100-ms kill trigger, periodic health and silence deadlines.
That is not a controlled functional conformance test. The surface fixture README
already describes a deterministic clock; the adapters do not currently provide
an explicit harness-controlled implementation of it.

## Decision

Introduce an explicitly opted-in, test-only clock-control binding for surface
fixture adapters. It is out of band, never a surface protocol message and never
available on production surface connections. Both direct and in-process variants
must drive the same Surface receive/tick semantics and real durable store.

The harness and adapter must serialize and acknowledge clock advances before
processing their associated stimuli. Advances use case `at_ms` boundaries, are
monotonic, and preserve scheduled local fixture events (including the 100-ms kill).
Ordinary frame waits do not advance logical fixture time. The control channel must
be bounded, fail closed on malformed/retrograde time, and be closed/reaped on all
failure paths. Existing non-opted-in implementations retain existing semantics.
No case-name-based reply synthesis or skipping unsolicited frames is permitted.

Use a separately named bounded runner I/O watchdog for this fixture binding and
retain a whole-case watchdog. These are test infrastructure liveness bounds, not
protocol latency promises. Do not multiply silence durations or action deadlines,
retry failed exchanges, or ignore EOF, unexpected health, or missing messages.

No surface wire/schema or production timing requirement changes are proposed.
Functional deterministic-clock results must be reported separately from live-time
qualification. Retain real-clock SDK/transport tests for expiry, health, silence,
and independent kill. Any newly exposed live-time safety defect requires its own
regression and must not be hidden by deterministic tests.

## Alternatives

- Remove mandatory syncs: rejected; violates durable admission/fencing/recovery.
- Batch terminal and sequence records only: retained as an independent improvement,
  insufficient for negotiation and rejected-action persistence.
- Increase every `within_ms` alone: rejected; conflates runner stalls with protocol
  time, introduces expiry/health ordering changes, and provides no clock control.
- Retry CI until green: rejected; hides a required-test flake and queue ejections.
- Replace real adapters with canned script responses: rejected; proves nothing
  about the actual implementation.

## Consequences

No wire compatibility or production clock changes. The opted-in harness binding
and both reference/mock adapters change together. The existing startup-readiness
negative control must continue to detect absent readiness. Deterministic surface
suite passes do not establish live-device latency or a universal scheduler bound.
The prior 75-ms residual proof remains part of the record.

## Verification

Implementation and all evidence below are REQUIRED and currently UNEXECUTED:

- A regression demonstrating current 200-ms failures with identical >200-ms
  durability stalls at negotiation and post-kill rejection; corrected runs use the
  actual durable store and compare expected frames, not simulated API responses.
- Clock binding tests for monotonic/invalid advances, acknowledged processing
  order, explicit kill scheduling, teardown, and unchanged non-opted-in behavior.
- Hung/missing responder, partial frame, unexpected-message and EOF negatives;
  whole-frame and whole-case watchdogs still fail closed.
- 50 consecutive full surface iterations under owned CPU contention, including
  read-config, action-accept-outcome, dedup-terminal and kill-latched; report both
  CPU-only and storage-perturbed evidence and preserve failing controls.
- 20 consecutive complete registered conformance runs in exact-head CI;
  surface-readiness must also pass. Include run45340588562 as the baseline failure.
- Independent exact-head review, all required checks green, queue merge, and
  postmerge main checks green before task completion.

## Approval

Proposed, not accepted. The earlier independent review of d74a69f covers the
barrier reduction only. A new exact-head independent review must assess this
clock/budget decision and its implementation/evidence before landing.
