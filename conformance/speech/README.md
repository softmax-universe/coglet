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
