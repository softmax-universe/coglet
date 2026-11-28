# ADR-0044: M2b real-Mac acoustic proof — measured distributions and acceptance thresholds

- Status: Accepted for the M2b real-Mac acoustic gate (Mac mini, tone/scripted-ASR stand-ins; see Limits)
- Date: 2026-11-27
- Authors: Coglet face-mac maintainers (card t_01a2084f47ae5140)
- Related PR: the publishing PR of branch `feat/m2b-acoustic-proof-mini-t_01a2084f47ae5140`
- Supersedes / superseded by: none

## Context

M2b step 3 asks for real-Mac acoustic evidence of the `face-mac` adapter: push-to-talk
(PTT) latency, barge-in latency and heard-truncation accuracy, measured first and gated
second. There are no inherited numbers (the Mindister Face handover is void). The
conformance speech timelines in `spec/fixtures/speech` are frozen; this ADR adds
real-hardware thresholds on top of them and changes none of them.

Test host (David, 2026-11-24): the Mac mini `daveey-mini-1` (macOS 27.2.1), not the
MacBook Pro. A mini has no built-in microphone; the rig is the mini's output (`LG
ULTRAWIDE` display speakers) into an `Unknown USB Audio Device` USB microphone, at system
output volume 90 (restored afterwards). A Mac mini is therefore not an acoustic AEC
target and says nothing about laptop microphones or echo cancellation (AEC off here).

## What was run

`scripts/acoustic/run.sh` builds `surfaces/face-mac` product `face-acoustic`, wraps it in
an ad-hoc-signed `.app` (macOS grants microphone TCC to bundles, not bare SSH
processes), and runs the **real** `FaceSurfaceHost`, `FaceSurfaceBackend` and
`NativeFaceDuplex` (real `AVAudioEngine` capture and playback) from the frozen
`spec/fixtures/speech/timelines.json` shapes (`play`/`barge_in`, `ptt`). The runtime
peer is simulated in-process (hello reply, grants, 500 ms health frames). Acoustic ground
truth comes from the microphone, not from the adapter's own counters.

Declared test seams — honest limits of what this proves:

| Seam | Stand-in | Not proven |
|---|---|---|
| TTS (`acoustic-tone`) | five 500 ms tones (1000, 1400, 1800, 2200, 2600 Hz) with 100 ms gaps and aligned spans; one frequency per word so room reverberation cannot merge neighbours | any real voice, real synthesis latency |
| ASR (`acoustic-scripted`) | the timeline's scripted partial/final text | real recognition latency or accuracy |
| user voice | a second `AVAudioEngine` plays a 3400 Hz tone at a scheduled host time | real speech, mic-to-ear acoustics |
| AEC | off | echo-cancelled barge-in; laptop route switching; hot-unplug |

The harness is a test tool, exported as the macOS-only Swift package executable
product `face-acoustic` so the proof runner can build it. It is not bundled in the
installed product apps, opens no network route, and is not a deployed adapter.

Measurement method. Each session first calibrates: per-frequency noise floor
(p99 of a quiet second) and tone level (p10 of each tone through the speaker→mic loop);
detection threshold = half the calibrated tone level (−6 dB). A session refuses to
measure (exit 4, `acoustic_coupling_ok=false`) if any tone has less than 12 dB
SNR over noise; this fired once on a re-run attempt (SNR 11.77 dB at 1000 Hz;
`rerun-refused-by-coupling-gate/`) and was retried in a quieter moment. Runs whose
pre-play lead-in already exceeded a threshold are excluded as noisy (0 in both sessions).

Per-run metrics (raw: `docs/acoustic/m2b-mini-2026-11-27/*/runs.jsonl`, 5 ms
decimated envelopes included):

- `ptt_press_call_ms`: duration of `beginCapture` (60 per session, every barge run presses PTT).
- `ptt_press_to_first_capture_ms`: PTT press → first microphone frames at the recognizer.
- `ptt_release_to_end_event_ms`: release → `speech.end` event.
- `stimulus_scheduled_to_capture_ms`: scheduled user-voice onset → onset in captured audio
  (the capture-path latency while PTT is open).
- `play_to_acoustic_onset_ms`: `speech_chunk` act → tone audible at the mic.
- `barge_to_acoustic_stop_ms`: PTT press during playback → audible output falls below 25%
  of its pre-barge level (≥3 ms).
- heard truncation: the adapter's `speech.barge_in` `played_ms` is turned into credited
  whole words (500 ms word + 100 ms gap spans); ground truth is the number of words whose
  own frequency stayed above threshold for ≥97% of their window. Errors: `played_ms −
  (acoustic stop − acoustic onset)`; over-credit = credited words > acoustically heard
  words; `word_exact` = credited == heard.

Sessions (seeds recorded in `env.txt`, git sha `fcb83a3a`, tree clean): barge offsets
{~100, ~1000, ~1500, ~2100} ms × 15 reps = 60 barge runs, 20 PTT capture runs and 20
stimulus runs per session.

## Measured (session `measurement/`, seed 31)

| Metric | n | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| PTT press call (ms) | 60 | 0.66 | 1.66 | 1.73 | 1.80 |
| PTT press → first capture (ms) | 20 | 4.21 | 5.55 | 6.21 | 6.37 |
| PTT release → `speech.end` (ms) | 20 | 0.25 | 0.34 | 0.57 | 0.63 |
| scheduled voice → captured onset (ms) | 20 | 20.87 | 21.89 | 23.07 | 23.37 |
| play act → acoustic onset (ms) | 60 | 34.06 | 42.65 | 42.74 | 42.85 |
| barge press → acoustic stop (ms) | 60 | 55.58 | 114.33 | 115.83 | 116.35 |
| \|played_ms − acoustically heard\| (ms) | 60 | 17.78 | 79.60 | 81.54 | 83.85 |

Heard truncation: 60/60 runs word-exact, 0 over-credited. Conformance (event kinds and
`discarded` count equal the frozen timeline; collapsing the adapter's per-tick
`speech.position`): 60/60 barge, 20/20 capture. 0 missed stimuli. Signed error
`played_ms − heard` ranges −83.85 to +10.3 ms (p50 −17.8): the adapter's `played_ms`
leads the audible tail rather than over-reporting it (conservative: nothing is credited
that was not heard).

Barge-stop is bimodal (~40 ms vs ~110 ms) in the pilot samples: barges landing in
the 1400 and 1800 Hz words stop slower at the mic than the others, consistent with
frequency-dependent room ring-down on this rig rather than adapter latency (the render
counter stops at the press, −6 to 0 ms). It is not decomposed further here; the
threshold below covers the observed worst case with margin.

The earlier pilot sessions (a 1 kHz-only design whose heard-word ground truth merged
words in the room, over-credit 7/57; a ground-truth detector fixed once after a failed
re-run; per-frequency redesign) are kept under `superseded/` for transparency. The first
re-run against the first thresholds failed (`heard |error| max 52 > 35`) because
the detector, not the adapter, was wrong; the detector was then redesigned using short
sample runs (not the measurement or re-run sessions), and thresholds were derived only
from the final `measurement/` session.

## Decision

Threshold rule (`scripts/acoustic/analyze.py derive`): per metric, p95 limit =
ceil5(2 × measured p95); max limit = ceil5(max(2 × measured max, p99 + 20 ms)); sample
counts are lower bounds (`min_n`); heard-truncation word-exact rate ≥ 0.90 and
over-credited runs = 0; conformance 100%; missed stimuli 0. Rationale for 2×: a single
Mac, one room and one session do not support a tighter claim; a gate must catch
real regressions (a doubling) without flaking on room drift.

Accepted M2b real-Mac acceptance thresholds (committed as `thresholds.json`):

| Metric | p95 ≤ | max ≤ | min n |
|---|---|---|---|
| PTT press call | 5 ms | 25 ms | 15 |
| PTT press → first capture | 15 ms | 30 ms | 5 |
| PTT release → `speech.end` | 5 ms | 25 ms | 5 |
| scheduled voice → captured onset | 45 ms | 50 ms | 5 |
| play act → acoustic onset | 90 ms | 90 ms | 15 |
| barge press → acoustic stop | 230 ms | 235 ms | 15 |
| \|played_ms − heard\| | 160 ms | 170 ms | 15 |

plus: heard-truncation word-exact ≥ 90%, over-credited runs = 0, timeline
conformance 100%, missed stimuli = 0, coupling SNR ≥ 12 dB (the session refuses
otherwise).

Re-run (`rerun/`, seed 32, same script, same sha, a separate session after the
measurement) met every threshold: `analyze.py check thresholds.json rerun/` → PASS
(barge stop p95 116.6 ms, heard error p95 80.3 ms, 60/60 word-exact, 0 over-credited,
80/80 conformant, 0 missed).

To re-verify: `scripts/acoustic/run.sh <out> --barge-runs 15 --capture-runs 20
--stimulus-runs 20 --seed N`, then `scripts/acoustic/analyze.py check
docs/acoustic/m2b-mini-2026-11-27/thresholds.json <out>`.

## Limits and follow-ups

- Thresholds are for this rig (mini + USB mic + LG speakers, volume 90, AEC off, tone
  stand-ins). They bound the adapter's own audio-path behaviour on a real Mac; they
  are not a claim about speech latency, a laptop's microphones/AEC, or user-audible
  quality. A MacBook / AEC proof, a real TTS/ASR session and hot-unplug recovery remain
  untested and need their own measurement and ADR.
- Barge-stop at 230 ms is dominated by room ring-down on two word frequencies; a
  measurement with a broadband stimulus or a closer microphone could tighten it.
- Single session per claim: the distributions are one room at one time. Only a
  repeated, independent re-measurement would tighten the 2× margin.

## Follow-up measurement protocol (threshold amendment pending)

The trusted `FaceSurfaceBackend` constructor can now select immutable
`bargeMode: .duck`; its default remains `.stop`. Unsupported duplex implementations
refuse duck mode. A native duck applies linear gain 0.25 before publishing
`reflex=ducked` and the matching `state=ducked` snapshot. Admitted audio is not
flushed or replayed, queued chunks remain at reduced gain, and later actual render
frames alone extend heard history. Gain resets on stream close, cancellation and
native teardown. This constructor is not a guest-writable configuration switch.
The separate optional energy-VAD duck can multiply the adapter gain; it stays off
in this PTT/tone proof. No real-ASR, voice-classification or AEC claim is added.

`--duck-runs N` drives the frozen `over-talk-duck` config and transcript inputs.
The frozen finish input is not injected: the real 3-second PCM must finish and emit
its successful outcome. Progress observations are projected to item/state
transitions across transcript events, preserving all other events, and compared
with the frozen ordering. Reflex, paired position snapshot, final state and
terminal status must match, not just event kinds.

The duck latency is PTT press to the first sustained fall below 0.625 of the
pre-barge level (the halfway crossing between gain 1 and 0.25), **not silence**.
Steady-state amplitude is measured 150–250 ms after the press. A second word's
700–850 ms window after acoustic onset independently checks sustained acoustic
output. Both level ratios must be within 0.10–0.45 and at least 12 dB above their
own quiet lead-in p99. These mode-discrimination guards reject no duck, stopped
output and noise-only output; continued render frames alone cannot establish
acoustic continuation. Missing/non-finite measurements fail. Version-2 thresholds
also require measured duck latency and complete duck conformance. Historical
version-1 gates remain stop/capture-only and cannot certify duck.

`--ringdown-runs N` adds separately labelled per-frequency direct-flush diagnostics:
2-second tones are cut after 700 ms, bypassing the speech backend but retaining
the same native graph, microphone and 25%-for-3-ms detector. Compare these tails
with actual timeline barges before attributing frequency-dependent delay to room,
speaker DSP, capture, or adapter work. These diagnostics are not timeline passes.
The existing full-tone stop metric is unchanged; no faster threshold has yet been
accepted from this follow-up. Raw measurement, a separately seeded rerun, numeric
thresholds and their rationale must accompany the final amendment.

Explicit `--input-uid` and `--output-uid` pin both the duplex graph and the separate
stimulus engine to the intended rig without changing the system default route.
`--list-devices` lists UIDs without opening audio. `--permission-status` and
`FACE_ACOUSTIC_PREFLIGHT_ONLY=1` inspect TCC without requesting permissions or
playing sound; `FACE_ACOUSTIC_SIGN_IDENTITY` can use an existing stable signing
identity for an isolated harness bundle. Do not overwrite a previous proof bundle
or request unrelated grants.
