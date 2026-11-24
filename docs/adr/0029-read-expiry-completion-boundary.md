# ADR-0029: Read expiry completion boundary

- Status: Accepted scope decision; implementation gated on independent review and docs-only merge
- Date: 2026-11-24
- Authors: Coglet runtime queued-read investigation
- Related PR: https://github.com/softmax-universe/coglet/pull/153
- Supersedes / superseded by: none

## Context

Owner card `t_01a20fd6d43dfc22` investigates a demonstrated adapter transport
head-of-line failure. The additional requirement originally asked for a negative
read reply to the guest. The owning orchestrator corrected that recipient in
comment 249549: surface reads are runtime/supervisor-only. This decision creates
no guest read observation.

- [Operation bindings](../../spec/capabilities/bindings.md) make reads
  runtime-issued adapter observations, not guest imports. A read reply cannot
  become an unspecified guest event.
- [ADR-0009 M-C](0009-spec-v0-freeze.md) makes `runtime.heard` a runtime-owned file
  boundary, not an inbox or synthetic guest event.
- [Adapter protocol](../../spec/surface-protocol.md) requires local timeout
  reporting with `deadline_exceeded`, discarding late replies, and delivery only
  to the requesting coglet. It does not define guest read-result delivery.
- [Speech history](../../spec/capabilities/speech.md) requires the heard projection
  to be persisted before reasoning consumes history. A timeout is not evidence of
  heard text and must not release reasoning with invented or stale history.

On main `3ac5bc7b2fe985540a85a30be8b415bdbcd5d6c5`, generic read expiry
queues a negative result for native `SurfaceSession::take_read_result`. The
runtime-owned heard-read path instead disconnects before queuing that result
(`surface_reads.rs`, `expire_reads`). `take_read_result` is a native supervisor
API, not a guest ABI import.

The Face investigation at PR #97 head
`c7dc5da51e132250459f57e2884dd46d7092f4ab` provides a paired real C-ABI
reproduction: the control completed three guest steps with equal replay; delaying
only two post-first-heard event commits by 600 ms each caused receive status -2
before guest step three. The reply was queued in the adapter worker behind event
frames; it had not reached the runtime. This demonstrates a sufficient failure
mechanism, not the cause of the original run-19 failure.

The raw report and patches are retained under the Face card's evidence directory,
`investigation21-logs.tar.gz`, in `investigation21-ordering/`. Its sleep-based,
environment-controlled investigation patch is not a production change or the
required deterministic regression test.

## Decision

Every expired read completes negatively to the requesting runtime/native
supervisor through the existing native completion path (`take_read_result` or
equivalent), including runtime-owned heard reads. Expiry must not silently drop
the result or disconnect before native completion can be consumed. The native
completion is not a guest event, guest import, guest step, or new guest wake.

Preserve exclusive issued deadlines and correlation tombstones for late replies.
Do not change wire schemas, guest ABI, or the heard-only reasoning fence. Expiry
must not fabricate successful heard materialization or release reasoning against
stale history. Retain fail-closed revocation/disconnect and authority checks.

The owning orchestrator accepted this scope on the owner card in comment 249549.
This ADR must receive independent review and land docs-only through the normal
merge queue before implementation begins. It clarifies the recipient and does
not amend the frozen specs.

### Ownership and diagnostics

The runtime owner implements negative completion, bounded non-private expiry
diagnostics, and deterministic tests at the runtime boundary on main. Record the
read correlation ID, issued runtime deadline, and tick that expired it. Report
queued inbound frames ahead of the reply only where that queue is observable;
unknown adapter queue depth must remain explicitly absent/unknown, never an
invented measured zero. Diagnostics contain no text, arguments, or frame bytes
and have bounded retention.

Face card `t_01a2084da345e698` owns ordering inside PR #97's Swift FIFO, which
is not on the main revision above. It consumes the merged runtime head and
requalifies its own original C-ABI proof, including the delay experiment and
twenty plain iterations. Runtime tests do not substitute for those executions
or change Face acceptance. Preserve event-to-event ordering and authority fences;
no ready reply may bypass a revocation/kill boundary or interrupt an ABI call.
Runtime draining alone cannot ingest adapter-owned bytes.

All delay hooks remain test-only. Use injected time or a deterministic test seam
to hold a reply past its issued deadline, not runner-speed-dependent sleeps or
production delay environment switches. Do not widen deadlines.

## Alternatives

### Literal negative guest reply

Rejected. This would require a new specified ABI observation, including its
schema, producer/recipient, correlation, authority epoch, materialization,
replay, bounded retention and wake policy. Existing action-outcome or `llm.reply`
formats cannot be repurposed. No such extension is authorized.

### Extend issued deadlines

Rejected. This hides latency without fixing mailbox ordering and changes the
contract. A new allowance cannot retroactively extend an issued read.

### Drain runtime input before expiry only

Insufficient for the demonstrated case: the reply is still in the adapter's
outgoing mailbox. Runtime receipt and adapter queuing are different boundaries.

### Remove the disconnect and release the heard barrier

Rejected without valid heard materialization. An expired observation is not proof
of what played; continuation with stale history violates the heard-only
reasoning requirement.

## Consequences

The runtime can expose an explicit negative native completion without inventing
a guest read interface. Runtime timeout reporting and adapter FIFO ordering are
separate responsibilities. The runtime fix does not establish physical Face
qualification, and no success is claimed for implementation by this document.

## Verification

The runtime implementation must commit these named tests:

- `queued_read_reply_control_without_delay`: deterministic control using the
  runtime boundary and original issued deadline.
- `expired_read_completes_negatively_to_supervisor`: paired test-only delay holds
  the reply past its deadline and observes one negative native completion.
- `expire_reads_does_not_disconnect_before_native_completion`: expiry leaves the
  existing native completion consumable rather than returning a disconnect first.

Include exact-deadline and late-reply controls, unchanged deadlines, correlation
tombstones, no fabricated heard update or unsafe reasoning release, bounded
privacy-safe diagnostics, and authority/revoke/kill controls. Distinguish unknown
adapter queue depth from measured zero. The negative cases must fail on the
unfixed runtime. Run the deterministic regressions in a required job; retain
twenty consecutive CPU-load passes as separate stability evidence, never a stress
loop in the required job.

The Face owner separately reruns its original named C-ABI proof on the merged
runtime fix, both the delay experiment and twenty plain iterations, and posts
the actual results on its card. Runtime acceptance still requires independent
exact-head review, required CI and merge. This docs-only ADR delivers none of
those implementation or physical proof results.

## Approval

Owning-orchestrator scope decision: owner-card comment 249549, 2026-11-24.
Independent exact-head review and normal CI/queue merge are still required before
implementation. This decision grants no automatic release of other held scopes.
