# Speech conformance case data

Normative profiles: [`speech`](../../spec/capabilities/speech.md) and
[`expression`](../../spec/capabilities/expression.md). JSON files use the shared
harness interface from PR #15. Harness role is runtime (surface-under-test).
All cases need `surface-base` from the surface suite and one `audio-<case-name>`
fixture. Unmapped fixtures must be reported SKIP, never PASS.

The audio fixture feeds native VAD/STT/playback callbacks using the named script in
[`timelines.json`](../../spec/fixtures/speech/timelines.json), NOT raw audio or
invented control messages. It uses virtual case-relative monotonic time and the
configured synthetic utterance/stream IDs. Playback begins on the accepted
speak-chunk. No provider/network call is needed. `over-talk-duck` configures duck;
only the first two cases configure capture/playback callbacks. In the late-partial
case require wire silence from the terminal end through 350 ms; the late callback
at 250 ms must not produce a frame. No periodic health before 1 second is needed
by these short, explicitly configured fixture scenarios.

- **barge-in-stop:** native stop before notification, position + cancelled outcome,
  then user partial and final. No queued output resumes.
- **over-talk-duck:** input continues while output is ducked; final transcript
  does not terminate playback; real completion yields a succeeded outcome.
- **late-partial-and-stale-revision:** one stale revision before end and one late
  partial after end are filtered, preserving exactly one final.

`spec/tests/test_surface_speech.py` validates the script's normalization oracle,
all typed payloads, conformance sends and exact expectations. This is fixture
validation, not an acoustic measurement, provider integration, implemented mock
or a real surface conformance pass. Expression action admission/outcome is covered
by the surface suite; body-specific visual appearance is a product gate.

## ADR-0010 fixture contract

Five additive cases use a single `speech-handoff-<case-name>` fixture label each.
It performs the ordinary authenticated surface-base binding, advertises history
and work operations with schema URIs from speech.schema.json, grants both audio
leases and all requested operations to the fixture coglet, and suppresses unrelated
health/provider notifications. The deadline/grant window is five seconds. These
are fresh isolated sessions: work epoch 1, grants epoch 1, snapshot revision 1.
Unmapped fixtures are SKIP, never PASS. There is no real provider/audio call.

- **heard-truncation:** synthesize the scripted 3,000 ms `Hello world` item on
  speak-chunk acceptance; report playing(0). At native playback 1,200 ms, feed
  user start/stop-reflex. Alignment is `Hello` ending 1,000 ms, ` world` ending
  3,000 ms. Report start, barge_in, stopped-position and action cancellation;
  speech.heard must return 1,200 ms and only `Hello`. The runtime's private
  conversation/journal projection must agree (semantic trace oracle covers it;
  this surface wire case alone does not prove runtime journal persistence).
- **work-cancelled:** stage the single INTENT from handoff.fixture; provider-owned
  output is already active (no speak action pending). At 1,200 ms interrupt it,
  emitting start/barge/position then work_done(cancelled). speech.work_state must
  show epoch 2/pending 0. Submitting its old result must fail not_found with no
  additional work result or speech-triggering frame during the quiet interval.
- **duplicate-call:** feed the same complete native request twice, with identical
  turn/call/arguments. Only one intent may emerge. The duplicate occurs in the
  first 25 ms quiet interval; work_state reports one pending call. Return the
  verified result, require one work_done and one action outcome, then silence.
- **mode-hands-free / mode-push-to-talk:** discovery-only fixtures advertise only
  the indicated empty-operation marker and an empty grants entries list. They do
  not start capture or imply microphone consent. Both markers at once or the wrong marker
  fail the exact entries assertion.

`build_speech_handoff.py --check` pins these cases and the semantic traces.
`test_speech_handoff.py` executes all five through harness.timeline with a
scripted frame reader, then repeats each with a corrupted safety field. This is
case-contract validation, not a production executor or real-surface pass.
