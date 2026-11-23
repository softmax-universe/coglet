# Speech capability profiles — 0.1

Imports [conventions](../conventions.md) and [surface protocol](../surface-protocol.md).
Decision: [ADR-0007](../../docs/adr/0007-surface-speech.md).
`speech.cddl` and `speech.schema.json` define the CLOSED typed payload/args
records used inside common envelopes. The surface still checks temporal rules
and grants; an arbitrary common payload map is not a valid speech message.
Native capture, echo cancellation, VAD and playback run outside the Wasm step.
No raw audio/base64 or provider credentials are carried by these messages.

## Discovery and placement

The `speech.control` entry advertises the five basic speech events (start,
partial, end, position, barge_in), the three basic actions (speak_chunk, stop,
backchannel), plus read `speech.config`. The provider operations and tool_call
belong only to the optional `speech.session` entry below. Each body implements the same semantic events/actions; no Mac,
Echo Dot or EO1 branch belongs in the face coglet. Expression is a separate
optional capability. A body MUST NOT advertise operations it cannot implement.
The face baseline requires speech.control and expression.control, including a
semantic LED/no-op renderer for bodies without a face. Persona and provider voice
selection belong to product configuration, not this protocol.

Read `speech.config` returns a snapshot of `speech-config`:
`{stt, tts, barge, session_mode}`. STT and TTS are each `{placement, route}`;
placement is `surface`, `runtime`, `hub` or `provider`; route is a nonempty opaque
configured route name, not a URL/token. `barge` is `stop` or `duck`; session_mode
is `text` or `speech_to_speech`. Configuration is installed by trusted operator/hub
policy before grant issuance, not by arbitrary coglet writes. Changes revoke the
old epoch/session and publish a new config snapshot before restarting capture.
Remote audio paths require explicit local privacy permission and authenticated
media transport configured outside this control protocol. Unavailable routes
report unavailable; no silent switch to a cloud/provider path. Regardless of
placement, the surface timestamps, fences and normalizes every resulting event;
reflex stopping remains local even if recognition/synthesis is remote.

## Events up

Every event is a common event sourced by the surface, with monotonically
increasing seq. IDs below use the common 32-hex correlation-id grammar. Capture
and playback `position_ms` refer to their own media timeline, not logical_time.

| kind | payload | Meaning |
|---|---|---|
| speech.start | `{utterance, position_ms}` | native VAD/hold-to-talk starts one utterance |
| speech.partial | `{utterance, revision, text}` | replacement full hypothesis, not a text delta |
| speech.end | `{utterance, revision, text, position_ms}` | terminal full transcript, possibly empty |
| speech.position | `{stream, chunk, item, played_ms, position_ms, state}` | actual rendered playback; state playing/ducked/stopped/finished |
| speech.barge_in | `{utterance, stream, item, played_ms, position_ms, reflex}` | already-applied stop/duck; reflex stopped/ducked |
| speech.tool_call | `{session, call, name, arguments}` | provider request, NOT executed tool |

Utterance IDs are unique across capture restarts. Revisions start at 1 and strictly
increase per utterance (end must exceed the latest partial). Start precedes partial
and end. Exactly one end closes an utterance. A late partial after end, or a lower/
equal revision, is discarded by the surface normalizer and MUST NOT reopen an
utterance or wake the coglet. Record the discard in local diagnostics without
forwarding private text. An event already in the runtime queue when end arrives
is processed in source-seq order, with the same closed/revision filter on replay.
Text is UTF-8, unnormalized, at most 16,384 bytes per transcript. Limits in schemas
are Unicode code points where noted; the UTF-8 byte limit is an additional check.
Duplicate playback reports are harmless; position_ms never decreases within a
stream. `item` identifies the speak item: it equals its speak_chunk/backchannel
action ID. Provider-originated items use a fresh native 32-hex ID (not an invented
action admission). `played_ms` is actual rendered media duration WITHIN that
item, starting at zero and nondecreasing per item; it excludes synthesized/queued
but unheard audio. On barge-in both events carry the same current item and final
stop/duck played_ms. This is independent of cumulative stream position_ms. A
transcript can therefore truncate the interrupted item without counting queued
text as heard. Work-intent and hands-free/PTT mode extensions are out of scope.

## Actions down

| capability | args | Completion |
|---|---|---|
| speech.speak_chunk | `{stream, chunk, text, final}` | succeeds after that chunk is actually rendered |
| speech.stop | `{stream}` | succeeds when queued/native output for that stream is silent |
| speech.backchannel | `{stream, cue}` | rendered acknowledgement; cue ack/thinking/agree |

`speech.speak_chunk` needs audio.speaker. Speech capture/duplex activation MUST
lease audio.mic and audio.speaker to the same coglet; split-owner duplex is not
supported by this profile. Start/partial/end go only to the mic owner, position
only to the speaker owner, and barge_in only to their common owner. Without an
active stream, user speech emits start but no barge_in. Chunk starts at 0 and increments by 1
for each accepted chunk in a stream. At most 64 admitted chunks per stream;
noncontiguous or reused stream/chunk with a new action ID is conflict. Additional
speak chunks after final/stop/reflex-stop also fail conflict. If a chunk fails or expires,
stop that stream and cancel all later admitted chunks rather than play a gap.
Retain closed-stream tombstones for the surface's action-retention interval;
after that interval stop may return not_found. Callers MUST allocate fresh random
stream IDs forever; the bounded surface ledger does not certify global uniqueness.
An unfinalized idle stream closes after 60 seconds without an admitted chunk.
Text is
nonempty and at most 4,096 UTF-8 bytes. `final=true` closes the input side after
that chunk; no more chunks may be admitted. Stream IDs are never reused. Accepted
chunks play in chunk order; no mixing two coglet streams on one speaker. A
backchannel starts its own stream (chunk 0, implicitly final), takes the same
speaker lease, and has one terminal outcome. It fails conflict if its stream ID
is already known or another stream is active; it does not mix with or overtake
speech. A different speak stream likewise fails conflict while another is open.
Every action also has a deadline and admission/outcome
under the surface protocol. Deadline expiry stops its audio, not just a callback.
No successful outcome merely because TTS synthesis completed remotely.

Stop requires ownership and cancels queued/playing actions with cancelled outcomes;
already finished actions are unchanged. Unknown stream is not_found; stopping an
already stopped known stream succeeds idempotently. Native barge-in reflex MUST
NOT await this action or any coglet step. On detected user speech while output
is playing, the surface applies configured stop/duck immediately, emits start then
barge_in with actual output position, and reports position. With stop, all admitted
chunks for that stream get cancelled outcomes; with duck, they remain admitted
and may continue at reduced gain until the coglet stops them. A stopped stream
cannot resume; the coglet may create a new stream for the remaining text. On
barge-in no automatic replay of old text is permitted. During over-talk (input
while output continues under duck) echo cancellation distinguishes captured user
speech from self-output; input finals remain available to the coglet independently
of output completion. Conformance timelines prescribe ordering, not an unmeasured
hardware latency threshold; real acoustic timing needs a separate measured gate.

## Provider speech-to-speech sessions

Optional `speech.session` entry advertises actions `speech.session_open`,
`speech.session_close`, `speech.tool_result`, and event `speech.tool_call`.
Open args are `{session, route}`; route selects an allowlisted provider preset,
never a credential or arbitrary endpoint. Open requires audio.mic and audio.speaker,
plus explicit speech.session_open grant, and config session_mode=speech_to_speech.
Only one active provider session per microphone/speaker pair. Successful open
means the session is ready; its long-lived conversation is not an action kept
pending forever. Close `{session}` stops capture/output and closes transport,
cancels outstanding calls and fences late provider callbacks. Config text-mode
and a provider session cannot simultaneously own these resources. Session-originated
speech is normalized into the same start/partial/end/position/barge_in events.

Tool names are common method-names. Call IDs are unique per session. Surface
normalizes allowlisted requests into speech.tool_call, delivered only to the
session owner; duplicates are not executed again. The coglet routes proposed work
through ordinary hub policy/grants. Neither provider arguments nor tool names
confer authority. `speech.tool_result {session, call, ok, value}` supplies one
profile-map result (on failure value is a common error); only the owner can reply,
and only once for a live pending call. Unknown/closed calls fail not_found,
conflicting duplicate results conflict. Close, revoke, disconnect, expiry or kill
terminates media and prevents new tool-call events. Tool results may contain only
the data permitted by the session's explicit privacy grants.

## Fixtures

`fixtures/speech/cases.json` covers typed args/events and config; `timelines.json`
covers native normalization, barge-in, over-talk and a late partial. Executable
wire-level conformance stimuli are in `conformance/speech/`. No audio waveform or
real acoustic success is claimed by these control-plane fixtures.
