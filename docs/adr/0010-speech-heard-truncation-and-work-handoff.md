# ADR-0010: Heard-audio truncation and speech work handoff

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet speech profile worker
- Related PR: pending publication
- Supersedes / superseded by: extends ADR-0007; no replacement

## Context

[The design of record](../design/coglet-runtime.md#face-full-duplex-conversation)
requires heard-only conversation history, cancellable voice-to-agent work and
surface-owned hands-free/push-to-talk modes before the v0 freeze.

PR #9 already supplies `item` and authoritative item-relative `played_ms` in
`speech.position` and `speech.barge_in`, immediate native stop/duck, coglet-owned
drop/resume decisions, provider `tool_call`/`tool_result`, session-owner grants,
and duplicate-call suppression. We retain those rather than inventing replacement
playback events, a second provider transport or another execution authority.
Missing are mandatory heard-history semantics, bounded epoch-fenced work and
mode discovery. Existing 0.1 closed records remain unchanged; additions use
optional capability discovery, not new required fields in existing records.

Prior art, read at Companion `d67f962`:

- [`realtimes.nim` L844–872](https://github.com/softmax-universe/companion/blob/d67f962/src/companion/realtimes.nim#L844-L872): `interrupt(playedMs)` cancels generation and truncates the provider item at `audio_end_ms`.
- [`processWork` L378–430](https://github.com/softmax-universe/companion/blob/d67f962/src/companion/realtimes.nim#L378-L430), plus `cancelWork` and `completeWork`: stage before dispatch, deduplicate call IDs, fence callbacks by epoch; caps are four pending calls, 8,192 request bytes and 32,000 result bytes.
- [`voices.nim`](https://github.com/softmax-universe/companion/blob/d67f962/src/companion/voices.nim): `VoiceInstructions`; `realtimes.nim` `setHandsFree` configures native turn detection. Instructions alone are insufficient outcome evidence.

Companion's own code/docs are Softmax IP (David's licence clarification; see the
design document). This change nevertheless copies no Companion implementation:
the schemas, trace oracle and tests are newly written against Coglet envelopes.

## Decision

1. Make rendered position authoritative for the conversation's heard record,
   including a 1,200 ms interruption of a 3,000 ms item. The surface exposes
   `speech.heard` as an optional read under `speech.history`; it returns
   `{item, played_ms, text}`, with only conservatively time-aligned heard text.
   No proportional string slicing. Unknown alignment yields empty text. The
   runtime journals the heard projection before subsequent reasoning. Immutable
   action/event audit records remain intact but are not conversation history.
   Duck snapshots may advance only on later actual playback reports.
2. Optional `speech.work` normalizes both provider and text-turn handoffs as
   `speech.work_intent {turn, call, epoch, name, arguments}`. The existing provider
   call is mapped once, not emitted again as an independently executable tool.
   Work is staged before dispatch; policy/grants still decide execution. A read
   `speech.work_state` exposes its epoch/pending count. `speech.work_result`
   actions submit verified completions; `speech.work_done` events report the
   accepted result or cancellation. Use common envelopes and errors unchanged.
   Barge-in fences the epoch before cancelling pending work, including with duck;
   late results cannot trigger speech. Limit pending work to four, deterministic
   CBOR arguments to 8,192 bytes and result values to 32,000 bytes. Keep a bounded
   128-call dedup ledger without eviction in an epoch; exhaustion fails closed.
   Work cancellation is not rollback of already applied real-world effects.
3. Advertise exactly one active capture-mode marker, `speech.hands_free` or
   `speech.push_to_talk`, as a capability entry with empty operation lists. Mode
   changes use the existing trusted config/revocation path, never provider text.
   They cannot grant microphone access or silently enable hands-free capture.

The speech spec defines ordering, rejection, retention and privacy details.
No common envelope, ABI journal format or native audio transport is replaced.

## Alternatives

- Merely cancel generation: leaves unheard text in model context.
- Treat synthesis completion as playback: misstates what the user heard.
- Copy old provider calls into a second executor: duplicates real-world effects.
- Speak a model's predicted result: bypasses result evidence and policy.
- Add required fields to existing 0.1 records: breaks closed-record compatibility.

## Consequences

Older peers keep the original basic profile. Extension-aware peers require the
advertised optional operations; absent history/work support is not inferred.
Surfaces need conservative media/text alignment and a fenced work ledger. Private
results/history remain owner-only and subject to the existing privacy grants.
SDK generation must include the new shapes. The work ledger is not an exactly-once
claim across loss of durable state. Native stopping never waits for work or Wasm.

## Verification

`spec/tests/test_speech_handoff.py` checks positive and negative heard-history,
work-cancellation, duplicate-call, stale-result, byte-budget and outcome-evidence
traces under `spec/fixtures/speech/handoff.fixture`. The existing dual-schema
suite checks new typed goldens against actual JSON Schema and reference CDDL.
New `conformance/speech` cases drive the same operations over the real harness;
scripted frame-reader tests exercise passing and corrupted sequences. These are
case-contract tests, not real audio, provider integration or device conformance.

## Approval

Pending independent exact-head review and green CI; the reviewer merges the PR
before completing this card and releasing the v0 freeze dependency.
