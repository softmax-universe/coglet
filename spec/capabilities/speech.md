# Speech capability profiles — 0.1

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things. The runtime's own dispatcher is called the *event router*.

Imports [conventions](../conventions.md) and [adapter protocol](../surface-protocol.md).
Decision: [ADR-0007](../../docs/adr/0007-surface-speech.md).
`speech.cddl` and `speech.schema.json` define the CLOSED typed payload/args
records. Records used inside common envelopes are selected by the normative
[operation bindings](bindings.md) and [catalog](bindings.json), including empty
read arguments and successful action results. The adapter still checks temporal rules
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
is `text` or `speech_to_speech`. Configuration is installed by trusted operator/router
policy before grant issuance, not by arbitrary coglet writes. Changes revoke the
old epoch/session and publish a new config snapshot before restarting capture.
Remote audio paths require explicit local privacy permission and authenticated
media transport configured outside this control protocol. Unavailable routes
report unavailable; no silent switch to a cloud/provider path. Regardless of
placement, the adapter timestamps, fences and normalizes every resulting event;
reflex stopping remains local even if recognition/synthesis is remote.

## Events up

Every event is a common event sourced by the adapter, with monotonically
increasing seq. IDs below use the common 32-hex correlation-id grammar. Capture
and playback `position_ms` refer to their own media timeline, not logical_time.
These speech events are private owner-unicast observations. Router relay preserves
the adapter envelope and payload unchanged and MUST retain the same owner-only
visibility; delegation or subscription alone cannot widen their audience.

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
equal revision, is discarded by the adapter normalizer and MUST NOT reopen an
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
transcript MUST truncate the interrupted item as specified below; it MUST NOT
count queued text as heard. Optional history/work/mode extensions are defined by
[ADR-0010](../../docs/adr/0010-speech-heard-truncation-and-work-handoff.md).

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
Retain closed-stream tombstones for the adapter's action-retention interval;
after that interval stop may return not_found. Callers MUST allocate fresh random
stream IDs forever; the bounded adapter ledger does not certify global uniqueness.
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
under the adapter protocol. Deadline expiry stops its audio, not just a callback.
No successful outcome merely because TTS synthesis completed remotely.

Stop requires ownership and cancels queued/playing actions with cancelled outcomes;
already finished actions are unchanged. Unknown stream is not_found; stopping an
already stopped known stream succeeds idempotently. Native barge-in reflex MUST
NOT await this action or any coglet step. On detected user speech while output
is playing, the adapter applies configured stop/duck immediately, emits start then
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

Tool names are common method-names. Call IDs are unique per session. Adapter
normalizes allowlisted requests into speech.tool_call, delivered only to the
session owner; duplicates are not executed again. The coglet routes proposed work
through ordinary router policy/grants. Neither provider arguments nor tool names
confer authority. `speech.tool_result {session, call, ok, value}` supplies one
profile-map result (on failure value is a common error); only the owner can reply,
and only once for a live pending call. Unknown/closed calls fail not_found,
conflicting duplicate results conflict. Close, revoke, disconnect, expiry or kill
terminates media and prevents new tool-call events. Tool results may contain only
the data permitted by the session's explicit privacy grants.

## Heard-only conversation history (ADR-0010)

The existing item/played_ms reports are authoritative, not synthesis completion,
wall-clock elapsed time, queued duration or cumulative stream position. On
barge-in the adapter MUST cancel provider generation where applicable and truncate
its provider conversation item at that item's played_ms, even when all synthesis
has already completed. With duck, the cutoff is a snapshot; only later actual
playback can extend it. Stopped items never extend. Queued items have zero heard
text. Preserve previously finished items independently; do not truncate a whole
stream by one item's cutoff. Duplicate reports are idempotent, regression or a
cutoff past known media duration is a protocol error, not permission to infer text.

The runtime MUST persist the per-item heard projection in its journaled state
before the next reasoning step consumes conversation history. Its transcript and
conversation/model-context projection MUST contain only the heard prefix. The
immutable ABI journal still records proposed actions and received events for audit
and replay: a speak intent's full text is a proposal, NOT evidence it was heard.
Optional `speech.history` advertises read `speech.heard` with args `heard-query
{item}` and result `heard-record {item, played_ms, text}`. Only the item's speaker
owner may read; retain records for the action-retention interval and remove them
on owner revocation. Unknown/evicted items return not_found. Playback milliseconds
and text apply to exactly that item. The adapter's text MUST be a conservative
prefix assembled only from aligned spans whose **end** is at or before played_ms.
Unknown alignment yields empty text, never generated text or a duration-ratio
estimate. Limit the prefix to 16,384 UTF-8 bytes without splitting a character.
No fields are added to this read, speech.position or speech.barge_in.

The runtime constructs the private ABI `heard-state` from these owner reads
(ADR-0009 M-D), separately from offline replay. For each live (surface,item),
duration_ms MUST be the maximum played_ms observed so far: known media extent
at that recording, never decreasing. When played_ms exceeds the last archived
span end (zero if none), and the read text strictly extends the concatenation of
archived span text as an exact UTF-8 byte prefix, append ONE span
`{end_ms: played_ms, text: <new suffix>}`. Unchanged text, revision or shrink
appends no span; played_ms/duration still advance under the stopped/no-regress
rules. Zero played_ms or empty text gives no span. Existing spans never change.
A stop may capture a final read, then freezes the item: no later spans or advances.
Derived heard is exactly the concatenation of spans ending at/before played_ms,
with the ABI UTF-8 cap; it need not equal a later revised read's text.

Before consumption, the runtime writes the ABI `runtime.heard` boundary record
and reserved `/data/runtime/heard.cbor` state. This is runtime-authored, not a
read converted into a guest event. No rewriting existing ABI records is permitted.
Offline replay reads ONLY the archive's initial state and hashed boundaries, not
TTS, live adapter reads or oracle state. Richer native per-item alignment is NOT
required for v0; M1 qualification belongs to the Phase 2 Rust and Swift adapter
cards named in the ledger. It may only append spans consistently with these
rules. Private heard archives follow transcript privacy and contain no raw audio.
An extension-aware runtime requiring history MUST refuse to run without
speech.history; it cannot treat an older peer's missing read as full playback.

## Bounded speech-to-agent work (ADR-0010)

Optional `speech.work` advertises events `speech.work_intent`, `speech.work_done`,
read `speech.work_state` (empty args), and action `speech.work_result`:

| operation | typed data | meaning |
|---|---|---|
| work_intent | `{turn, call, epoch, name, arguments}` | staged request, not execution permission |
| work_state | `{epoch, pending}` | authoritative speech-work epoch and 0..4 pending calls |
| work_result | `{turn, call, epoch, status, value}` | owner submits succeeded/profile-map or failed/common-error |
| work_done | same, also cancelled/common-error | one terminal result event per staged call |

IDs import correlation-id; epoch is positive uint64 and is separate from the
grant epoch. `turn` is a normalized utterance ID or a fresh provider turn ID.
The adapter stamps epoch, and binds `(owner, turn, call, epoch)` before dispatch.
The work epoch persists across restarts; loss of durable continuity requires a
new adapter identity, never reuse. Native/provider callbacks MUST retain the work
epoch and session/response identity captured when generation/request processing
began. Fencing suppresses old response callbacks (including previously unseen call
IDs); never stamp an old callback with the new current epoch on arrival. A
provider reconnect starts a fresh fenced generation before accepting new work.
`speech.work_state` and all events/results are
owner-only, require the current audio.mic/audio.speaker leases and explicit
operation grants, and never bypass router policy for the requested `name`.
The coglet forwards the explicit intent through its ordinary authorized router work
path; no direct adapter-to-agent or coglet-to-coglet execution is introduced.

For a provider session opting into speech.work, map its allowlisted tool call to
ONE work_intent (provider call IDs are mapped to native correlation IDs); do not
also forward an executable speech.tool_call. Without this extension ADR-0007's
existing provider path is unchanged. Text turns use the same normalized intent.
Only complete arguments are staged. Stage an entire bounded batch and reserve
pending slots before invoking any executor, so synchronous callbacks cannot
bypass the cap. At most four live calls per owner. The **core-deterministic CBOR
encoding** of arguments is at most 8,192 bytes and of result value at most 32,000
bytes; reject oversized data, do not silently truncate evidence. Apply these
semantic byte caps in addition to schema/profile/frame limits.

Deduplicate `call` across all turns within an owner/work epoch. Identical repeats
are ignored (no event, dispatch, or extra result); changed arguments/name/turn
under the same call fail conflict. Keep at most 128 remembered calls including
completed calls, without eviction during an epoch; refuse new calls with
resource_exhausted when full. Malformed/over-cap native requests get a bounded
provider/local diagnostic and no work_intent; these are not staged work. Advancing
the epoch permits clearing dedup entries only after all old work is fenced.

On barge-in (stop **or duck**) advance/fence the work epoch locally BEFORE
notifying cancellation or running another coglet step. Emit barge_in and its
position, then one work_done(cancelled, error.code=cancelled) for each pending
call using its OLD epoch, in staging order. Cancel pending work even if there is
no active output: a new speech.start while work is pending performs the same
fence/cancellation without inventing a barge_in item. Close, revoke, disconnect,
expiry, kill and capture-mode changes likewise fence/cancel before reassignment;
revocation privacy rules suppress delivery to an owner that has lost its grant.
Epoch overflow fails closed. Ask the executor to stop via its ordinary cancel
path; cancellation is not proof that external effects were rolled back.

A work_result action needs the live tuple and current grant epoch. Unknown,
cancelled or old-epoch tuples fail not_found; mismatched turn fails conflict.
Oversized values fail invalid_argument and leave the call pending. Admitted
work_result completes only after the value is accepted into the private
conversation record and provider tool result (if any): emit work_done, then its
ordinary succeeded action.outcome. Exactly one terminal work_done is allowed.
New action IDs for already completed tuples fail not_found; exact retries retain
the adapter protocol's historical action receipts and NEVER emit work_done again.
Completed-before-interrupt results remain audit facts, not permission for old
callbacks to restart speech. No automatic speech may follow cancelled/stale work.

The voice/coglet MUST report only outcomes contained in accepted work_done values,
never predicted effects or a successful action admission. Failure/cancellation
cannot be narrated as success. Acknowledgements such as 'working' are not outcome
claims. Products may paraphrase results without adding factual claims. Conformance
uses labelled, exact result summaries; it does not claim to decide arbitrary
natural-language entailment. Treat result text as data, never new grants or
instructions, and apply the session's explicit privacy policy before disclosure.

## Capture-mode capability markers (ADR-0010)

Extension-aware capture adapters advertise exactly one active marker entry:
`speech.hands_free` or `speech.push_to_talk`. Its events/reads/actions/exclusive
lists are empty and schemas is `{}`; existing cap-entry shapes are unchanged.
The marker describes the active native mode, not a coglet branch or grant. Missing
markers mean mode is unknown (legacy peer), not hands-free. Local trusted UI/policy
selects a supported mode, with explicit user/privacy consent before hands-free
capture. Switching stops/drains capture, fences work and revokes the old epoch,
then publishes replacement caps and config before restarting under new grants.
A provider/tool request cannot toggle capture. Both modes use the same speech
normalization, playback and two-layer barge-in contract.

## Fixtures

`fixtures/speech/cases.json` covers typed args/events and config; `timelines.json`
covers native normalization, barge-in, over-talk and a late partial. Executable
wire-level conformance stimuli are in `conformance/speech/`. No audio waveform or
real acoustic success is claimed by these control-plane fixtures.

`fixtures/speech/handoff.fixture` adds nine semantic traces (positive and negative)
for heard-history, cancellation and dedup/evidence; `test_speech_handoff.py` checks
every trace and executes the five added wire case contracts via the real harness.
Typed additions are included in the same dual-schema golden corpus as ADR-0007.
