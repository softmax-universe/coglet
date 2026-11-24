# FaceMac and FaceAudioKit

## Face surface integration (draft)

`FaceSurfaceHost` is an embeddable MainActor host over `SurfaceSession`; the app
supplies authenticated runtime transport bytes via `receive`/`drain`, an exclusive
persistent `SurfaceLedgerStore`, an identity anchor, effective grants, and a
monotonic microsecond clock. Call `start`, negotiate, then explicitly
`activateDuplex(localConsent: true)` only after local permission. Grant replacement,
connection failure and the local off-switch stop native I/O. No guest may call
these trusted configuration APIs.

- Supply `NativeFaceDuplex(kit: FaceAudioKit(...))` to `FaceSurfaceBackend` for the
  actual shared graph. Do **not** also run `EngineFacePlayback`: that legacy helper
  owns another output engine. `renderedPlaybackFrames` is an integer render count;
  it is not an acoustic timestamp.
- Inject the configured `FaceSynthesis` and `FaceRecognition` routes. Each exposes
  its exact route label and whether it sends audio remotely. Route mismatches are
  `unavailable`, not fallbacks; remote routes need separate explicit
  `activateDuplex(localConsent: true, remoteAudioConsent: true)`. Labels in
  `speech.config` describe the injected implementations, not built-in Apple STT/TTS.
  Routes must remain immutable for an authority epoch; configuration changes require
  revoking the old epoch before reactivation. Authenticated remote media transport
  and provider credentials remain the embedding product's responsibility.
- Use `beginCapture`/`finishCapture` for hold-to-talk or `enableVoiceDetection` for
  the audio kit's RMS gate. Recognition callbacks retain their original utterance
  and session custody. Capture is bounded to 60 s, then at most 5 s finalization
  after release (repeated release does not extend it). Failure, timeout or replacement
  closes with an empty final at the next revision, never a guessed partial-as-final.
  A new start follows the previous end. Revocation instead suppresses private
  delivery. Synchronous final/failure and late callbacks cannot reinstall a retired
  native handle. Saturating diagnostics count failures/discards without private text.
  Raw PCM is passed to recognition only, never on the wire.
- Before `start`, optionally `attach(link:)` using `FaceSocketLink` over an SDK
  authenticated socket or `FaceEmbeddedLink(abi:)` over installed runtime symbols.
  The native timer drives both links; the latter preserves would-block byte identity.
  The host bounds queued messages and services at most 64 incoming messages per tick.
  Attached links own output consumption (`drain` returns empty). Fatal transport
  errors stop native media. A grant-epoch change with blocked/partially written
  old-epoch bytes disconnects rather than releasing private backlog.
- A native 20 ms task pumps health/deadlines before playback/capture and drains
  cancellations. Guest execution must stay off MainActor. `localOffSwitch` stops
  audio before writing the durable kill latch; the wire cannot clear it.
- Accepted stream IDs are reconstructed as closed tombstones from the authoritative
  SDK action ledger after restart, never by replaying effects. Rejected actions
  do not create tombstones. The bounded SDK retains actions indefinitely, a
  strengthening of the minimum 24-hour window; full ledgers refuse new IDs.
- Advertised entries are baseline `speech.control` and `expression.control` only.
  Provider, work and history extension qualification are **not** claimed. The
  backend's aligned heard projection is tested locally but is not advertised.

## Compiled adapter simulator path

`swift build --package-path surfaces/face-mac --product face-mac` produces an
executable that requires `--simulator`. It runs the real `FaceSurfaceHost`, SDK
session, backend and normalization with injected render-counted PCM and recognition.
Its local JSON-lines driver is a test seam, not a wire capability or production
native route. It does not open audio hardware or select an Apple/provider voice.

Run the named black-box timeline tests from `sim/`:

```sh
FACE_MAC_EXECUTABLE="$PWD/../surfaces/face-mac/.build/debug/face-mac" \
  uv run --locked --python 3.12 pytest -q tests/test_face_mac_executable.py
```

`test_compiled_face_conformance_timeline` drives the frozen `barge-in-stop` and
`late-partial-and-stale-revision` timelines through that binary, checking actual
wire events, render frames, cancellation and late-update diagnostics. The configured
adapter supports stop only; `over-talk-duck` is not qualified. A third test proves
synthesis is not success, chunks remain ordered and stop cancels queued audio.

With the same executable path, run:
`cargo test --locked -p coglet-runtime --test face_mac_adapter -- --ignored`.
`compiled_face_barge_in_journals_cancelled_action_and_replays_offline` exercises the
production runtime wire session, receives the real adapter's 1200 ms barge/cancel,
asserts that exact cancellation in the archived inbox, reopens its WAL and checks
offline per-record hash equality without invoking a guest. Its initial speak action
is a runtime fixture proposal, **not** a Face guest emission. It does not establish
journaled guest consumption of capture, heard-history equality or guest-engine execution.
The macOS Swift workflow explicitly runs all three executable cases and this runtime
case, plus the separate Face controller's unit tests and Wasm build.

## Configured native local routes

`AppleLocalRecognition` requires already granted microphone and Speech permissions,
an available recognizer with on-device support, and sets
`requiresOnDeviceRecognition = true`. It accepts the shared graph's capture PCM;
it opens no microphone or network route and never prompts or falls back to a server.
Supply `FaceSurfaceHost.capturePositionMS` through its position closure, not wall time.
Its bounded mailbox copies only transcript snapshots across the MainActor hop.

`AppleLocalSynthesis(voiceIdentifier:)` selects an explicitly installed OS voice
and uses `AVSpeechSynthesizer.write` to collect bounded PCM (30 seconds maximum).
It does not call `speak` or create a second speaker graph. Conversion preserves
resampling phase across callbacks and fails closed on malformed/oversized data.
Word alignment is unknown (empty spans), not guessed. The backend's actual render
counter remains authoritative. Cancellation/replacement invalidates callback custody.

`Examples/LocalFaceEmbedding.swift` is compiled as `FaceMacEmbeddingExample` by
macOS CI. The app still owns permission UI, Info.plist usage descriptions,
sandbox entitlement, grants, identity/ledger and qualified runtime transport.
The CLI remains simulator-only; there is no native command-line permission UI.
Native route source and deterministic conversion/callback tests do not establish
installed-voice behavior, recognition accuracy, acoustic quality or runtime guest execution.

Remaining acceptance work: durable history retention/advertisement and heard-state
replay, production embedded-runtime qualification, final macOS CI and independent
exact-head review. `FaceEmbeddedTests` still uses an ABI fixture. The runtime now
hosts restricted transactional WASI; the additional named test
`persisted_face_wasm_emits_to_compiled_adapter_barges_reopens_and_reexecutes`
executes the actual Face Wasm with the compiled Swift adapter and independently
re-executes its archive (see `coglets/face/README.md` for the release-mode command).
It caught and fixed a source-sequence-zero admission bug: zero is a valid first
sequence, not an already-seen sentinel. No heard-state or C-ABI qualification is
inferred from this test. PR #79 has merged; the new native test still needs its
own macOS CI proof. This draft is not full M2b delivery or acoustic proof.

# FaceAudioKit

A reusable macOS 14+ duplex audio library, independent of the surface protocol.
`FaceMac` remains available; clients import `FaceAudioKit` for audio.

```swift
import FaceAudioKit

// Run on MainActor. The containing app supplies NSMicrophoneUsageDescription
// and the audio-input entitlement if sandboxed.
let audio = FaceAudioKit()
guard await MicrophonePermission.request() else { return }
try audio.startCapture()
try audio.enqueuePlayback(pcm16) // [Int16], mono 24 kHz
let captured = audio.readCapture()
let playedMilliseconds = audio.playbackPosition * 1000
audio.flushPlayback() // surface-layer barge-in
audio.stopCapture()
```

## Contract

- All public lifecycle/enqueue/read calls are main-actor serialized. One
  AVAudioEngine runs both directions; PCM16 queues hold 4 s capture and 30 s
  playback. Enqueue is all-or-nothing and throws on overflow. Capture drops an
  overflowing block and latches `captureFailed`; restart to clear it.
- UID selection (`AudioDevices.list`, `selectDevices`) is freshly implemented
  CoreAudio code. Set `AudioConfiguration.echoCancellation` to enable/disable AEC.
  Selection precedes VoiceProcessingIO activation. Enabled AEC must be reported
  active on both nodes with identical I/O client formats or start throws.
- Capture is converted from the processed device format to mono PCM16/24 kHz
  by a stateful AVAudioConverter, with preallocated output and bounded input.
  Callbacks never block on control-thread locks. Converter internals are Apple's;
  this is not a claim that AVAudioConverter itself is allocation-free.
- Flush advances discard and playback-generation cursors. A render overlapping
  a flush is muted if the generation changes before its final check. Already
  submitted hardware/engine buffers cannot be retracted: "instant" means the
  next render boundary, not zero acoustic latency.
- Configuration-change recovery stops the old graph and creates fresh queues,
  converter and engine. Generation-stamped notifications from old graphs are
  ignored. Queued playback/capture is deliberately dropped on rebuild; recovery
  failures leave `isRunning == false` and `lastError` set. No infinite retry loop.
- `playbackPosition` is cumulative rendered non-silence-queue frames / 24000,
  excluding underruns and flushed queue contents. It is not latency-compensated
  acoustic playback position. Input/output meters are latest-block normalized
  peaks, not accumulating maxima. Frames rendered with gain zero still count.
- `EnergyVAD` is a simple RMS energy gate with frame-based hangover, **not** a
  speech classifier. `readCapture` drives it and calls `onVoiceActivity` on the
  main actor; poll frequently to prevent overflow and timely ducking. Set
  `duckingEnabled` for gain reduction while active, or use the hook externally.
  No arbitrary user closure runs on the realtime thread.

## Tests and remaining proof

`swift test --package-path surfaces/face-mac` runs on GitHub macOS runners:
ring wrap/overflow/discard, concurrent SPSC sample ordering, playback flush /
generation / position / gain, 48 kHz stereo conversion, energy hangover, and
fake-engine configuration rebuild/stale-notification/failure/permission paths.
The production render/tap callbacks are constructed on MainActor and invoked
from a detached task with synthetic buffers, verifying actor-independent execution,
playback/underrun output, capture conversion and meters without hardware.

No hardware device is opened by these tests. Real MacBook Pro acoustic AEC
attenuation, route switching, capture/output latency, barge-in audible tail,
and hot-unplug recovery remain **untested** and belong to the later Face acoustic
proof card. Unit CI is not acoustic proof.

## Provenance

The queue/discard/generation and duplex graph design follows the MIT original
`softmax-universe/polyworld`, `experiments/voice/duplex.m` at
`1d1e7629eab74e0e88c2f83bbba34e3d9984e887`. See [MIT notice](NOTICE).
No Companion GPL `devices.m` or `permissions.m` code was used.
