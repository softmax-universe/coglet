# ADR-0029: Read expiry completion boundary

- Status: Proposed — implementation paused for the owning orchestrator's decision
- Date: 2026-11-24
- Authors: Coglet runtime queued-read investigation
- Related PR: see the PR containing this proposal
- Supersedes / superseded by: none

## Context

Owner card `t_01a20fd6d43dfc22` fixes a demonstrated adapter transport
head-of-line failure. Its additional requirement says an expired read must produce
"a negative read reply to the guest, never a silent drop." That recipient is not
compatible with the frozen v0 read boundary:

- [Operation bindings](../../spec/capabilities/bindings.md) explicitly make reads
  runtime-issued adapter observations, not guest imports. A new guest observation
  requires an ADR; a read reply cannot become an unspecified guest event.
- [ADR-0009 M-C](0009-spec-v0-freeze.md) makes `runtime.heard` a runtime-owned file
  boundary, not an inbox or synthetic guest event. No read response becomes a
  guest event.
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
API, not a guest ABI import. Calling that API a guest delivery would be false.

The Face investigation at PR #97 head
`c7dc5da51e132250459f57e2884dd46d7092f4ab` provides a paired real C-ABI
reproduction: the control completed three guest steps with equal replay; delaying
only two post-first-heard event commits by 600 ms each caused receive status -2
before guest step three. The reply was already queued in the adapter worker,
behind event frames; it had not reached the runtime. This demonstrates a sufficient
failure mechanism, not the cause of the original run-19 failure.

The raw report and patches are retained under the Face card's evidence directory,
`investigation21-logs.tar.gz`, in `investigation21-ordering/`. Its sleep-based,
environment-controlled investigation patch is NOT a production change or the
required deterministic regression test.

## Decision proposed

**Recommend the existing-contract interpretation:** change the owner card's
recipient from "guest" to "requesting runtime/native supervisor". Every expired
read produces an explicit bounded negative native completion, including
runtime-owned heard reads. Preserve the exclusive issued deadline, ignore late
replies using correlation tombstones, and do not change wire schemas or guest ABI.

The owning orchestrator must explicitly accept that interpretation before work
resumes. Acceptance of this proposal is not acceptance of literal guest delivery.
If guest-visible delivery is intentional, reject this recommendation and amend
this ADR with an exact guest contract before implementation. Do not silently
choose a new inbox message, reserved event, file format, or wake policy.

Following that scope decision, the implementation remains responsible for:

1. Prioritizing a ready read reply over unrelated queued adapter event sends,
   preserving event-to-event sequence and authority fences. Never interrupt a
   currently executing ABI call or cross a revocation/kill boundary to recover an
   obsolete read. Runtime-side draining alone cannot ingest adapter-owned bytes.
2. Explicit negative native completion on genuine expiry; no fabricated successful
   heard observation, no silent loss, and no reasoning-barrier release merely
   because a deadline elapsed. Preserve privacy and fail-closed behavior on
   revocation/disconnect. The precise durable lifecycle of the completion and its
   barrier must be tested before claiming this requirement is delivered.
3. Bounded diagnostics: correlation ID, issued runtime deadline, expiry tick and
   queued frames ahead of a ready reply. Queue observations belong at the adapter
   mailbox that owns them. The runtime must not report an invented zero for an
   adapter queue it cannot see. Distinguish absent/unknown from measured zero.
4. Test-only injected clocks/commit seams; no production delay environment switch,
   deadline widening, or runner-speed-dependent sleep assertion.

This document proposes a scope clarification, not permission to alter any frozen
spec. It intentionally contains no runtime or adapter implementation.

## Alternatives

### Literal negative guest reply

This needs a new specified ABI observation. The existing action-outcome or
`llm.reply` inbox formats cannot be repurposed. A follow-up revision would need to
name and schema the terminal message, identify its producer/recipient, bind
correlation/operation/epoch, define durable exactly-once materialization and
replay, bound retention and storage, and distinguish delivery from a guest wake.
It must also define how a guest can observe failure without allowing reasoning
against unmaterialized heard history. These decisions cannot be inferred from
"negative reply". No such extension is authorized by this proposal.

### Extend issued deadlines

Rejected. This hides latency without fixing mailbox ordering and changes the
contract. A new allowance cannot retroactively extend an issued read.

### Drain runtime input before expiry only

Insufficient for the demonstrated case: the reply is still in the adapter's
outgoing mailbox. Runtime receipt and adapter queuing are different boundaries.

### Remove the disconnect and release the heard barrier

Rejected without valid heard materialization. An expired observation is not proof
of what played, and successful continuation with stale history would violate the
heard-only reasoning requirement.

## Consequences

The recommended scope preserves all wire contracts and guest behavior. A native
negative completion is not a guest event, a guest step, or a physical Face proof.
The original card remains incomplete until its tests, diagnostics, fix, independent
exact-head review, required CI, merge, and named Face reruns are verified.

The Face FIFO implementation currently exists on unmerged PR #97, not on the main
revision above. The runtime owner must coordinate that integration explicitly;
merging a runtime-only change does not prove the pending Face transport changed.
Face acceptance remains with its own card.

## Verification

After the recipient decision, commit named tests and report actual names/results:

- A deterministic delay-disabled/delay-enabled queued-reply pair exercising the
  actual mailbox policy and native C-ABI boundary, failing on the unfixed code.
- Exact-deadline and genuinely late-reply controls that retain the original
  deadline, emit one negative completion and reject/ignore late results as
  specified; no fabricated heard-state update or unsafe reasoning release.
- Bounded diagnostic tests, including measured-zero versus unknown adapter queue
  depth and privacy (no text, arguments or frame bytes in diagnostics).
- Queue order/retry/full-queue and revoke/kill controls; event source sequence must
  remain ordered and an obsolete reply must not bypass an authority transition.
- Required-job execution of the deterministic regression; twenty consecutive
  passes under CPU load are separate stability evidence, not a stress loop added
  to the required job.
- The original named Face C-ABI proof on the fix head, both delay experiment and
  twenty plain iterations, with results posted to the Face card. Do not substitute
  unit-test passes for those executions.

No implementation test or Face rerun has been performed for this proposal.

## Approval

Pending owning-orchestrator decision on the recipient, independent review and
normal CI/merge gates. Proposed status does not amend the frozen specs, satisfy
the implementation acceptance, or authorize automatic release of a held task.
