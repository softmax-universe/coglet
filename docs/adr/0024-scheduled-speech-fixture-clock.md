# ADR-0024: Scheduled speech fixture time independent of runner load

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet contributors
- Related PR: this PR
- Supersedes / superseded by: extends ADR-0021, no production protocol change

## Context

Required Python run 45393639941 on PR122 times out in duplicate-call step 2.
The checked-in case uses 200 ms, not the 80 ms reported in the initial lead.
The duplex adapter uses wall time for both adapter deadlines and accelerated
media; its negotiation publishes capabilities only after configured() durably
installs grants. Disk/scheduler stalls therefore consume a protocol fixture's
receive budget, and can also reorder media callbacks relative to requests.
A disconnected client then kills the whole Unix supervisor accept loop.

ADR-0021 separates functional fixture time from bounded runner I/O watchdogs,
but its explicit at_ms clock cannot progress speech cases whose next expected
frame comes from a scheduled media callback rather than an explicit at_ms.

## Decision

Add an opt-in `fixture_scheduler = true` registry binding, requiring stdio and
`fixture_clock = true`. Existing registry bindings and all speech JSON cases
remain unchanged. This test-only mode runs the real Python adapter, codec and
durable ledger over length-prefixed wire frames, with a private clock socket.
It is never enabled in the ordinary duplex adapter or a production connection.

The harness advances time to explicit at_ms boundaries. For an expected frame,
it first consumes already buffered output, otherwise requests the next scheduled
callback no later than the step's existing within_ms deadline. If no frame is
produced by that logical deadline, the step fails. Silence advances all callbacks
through the silence interval and fails on any output, including EOF. Clock
advances process callbacks in chronological order; they never jump over callbacks.
Media acceleration remains 10x, as in the existing duplex simulator.

Private controls are newline JSON objects containing exactly `advance` or `next`
with nonnegative integral microseconds. Acknowledgements contain `time` and
`bytes` (cumulative flushed wire bytes). A wire input also receives an ack after
its actual dispatch and output flush. The harness drains wire output concurrently
and waits for the ack's byte watermark before deciding silence or advancing time;
a control ack alone does not prove wire delivery across separate channels.
Malformed, retrograde, oversized or inconsistent controls fail closed.

Runner operations retain ADR-0021's 3-second I/O watchdog and the existing
whole-case watchdog. These are liveness bounds, not substitutes for within_ms.
No retries, canned replies, swallowed protocol errors, durability removal or
blanket timeout changes are permitted. Ordinary real-time Unix transport tests
remain in place. A disconnected Unix client is isolated to its own scenario,
with ledger closure before accepting the next setup; unexpected failures still
fail supervisor teardown.

## Alternatives

- Increase every timeout: rejected; moves the race and changes timed media.
- Use tmpfs or remove synchronous ledger commits: rejected; does not eliminate
  scheduling races and stops exercising the actual durability path.
- Rewrite speech JSON with explicit media timing: rejected; unnecessary fixture
  churn and duplicates the adapter's schedule in each case.
- Retry failures or quarantine Python wholesale: rejected; hides the defect.

## Consequences

No wire schema, production timing, speech case payload or within_ms changes.
Functional conformance proves logical ordering, not live-device latency or a
universal scheduler bound. Real-time SDK and duplex wire tests still qualify
expiry, stop/callback fencing and native media behavior separately. Shared
harness edits are limited to this explicit optional binding.

## Verification

- Deterministic disconnect regression fails on the old supervisor, then a new
  scenario negotiates successfully with the corrected supervisor.
- Inject >200 ms negotiation/setup and dispatch stalls: unchanged speech cases
  fail under wall time and pass under the scheduled fixture clock.
- Missing responder, logical deadline exhaustion, silence/EOF/unexpected bytes,
  invalid clock controls and byte-watermark barriers remain hard failures.
- All speech cases, focused simulator/runner suites, 20 consecutive speech test
  passes under owned CPU contention on a pool worker, exact-head required CI,
  independent exact-head review and normal merge-queue landing.

## Approval

Proposed pending independent exact-head review of this decision and its evidence.
