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
  authenticated socket or `await FaceEmbeddedRuntimeLink.start(abi:binding:configuration:)`
  over application-supplied production symbols and immutable installed JSON.
  Configure/start/open/send/recv/close/stop are confined to one serial worker.
  MainActor accesses bounded copied mailboxes (128 messages and 8 MiB per direction);
  a full send mailbox copies nothing and requires retrying identical bytes.
  `close()` fences bytes immediately, never waits for a running guest; `shutdown()`
  is an optional asynchronous teardown attestation AFTER the native stop reflex.
  One process-wide Face supervisor lease prevents a stale close stopping a successor;
  failed termination retains that lease. The app must not supervise the same global
  runtime through another coordinator concurrently. The old synchronous
  `FaceEmbeddedLink` is deprecated and remains only a non-scheduling fixture seam.
  The native timer drives links.
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
- Baseline entries are `speech.control` and `expression.control`. To enable
  `speech.history`, pass a **separate** exclusive durable `historyStore` to the
  host and include `speech.heard` in the owner's speaker grant. For file storage,
  provision a second private `FileSurfaceLedger` directory, never the action-ledger
  directory. Default hosts do not advertise history. Provider/work stay unadvertised.
- `FaceHeardHistory` saves registration/alignment/render cutoffs before native
  effect or observation release. Duplicate cutoffs do not rewrite the snapshot.
  Records are bounded to 1024, retained without eviction during ownership (matching
  the SDK's indefinite action retention), and purged on custody revocation, including
  host teardown/replacement. Reopening the history object validates its snapshot;
  a new host purges the old epoch before any binding is activated. Invalid storage
  fails closed, never restores empty history. A failed save or purge latches the
  backend unavailable and prevents reactivation. Local stop silences native I/O
  before disk I/O. `speech.stop` and barge-in retain the frozen per-item prefix.
  Unknown item is `not_found`; foreign owner/epoch is `permission_denied`. Unknown
  native alignment still yields empty text; no full proposal/duration estimate leaks.

## Embedded-worker reply ordering

`FaceEmbeddedRuntimeLink` promotes a validated `read_reply` across only the
contiguous trailing run of unsent `event` frames in its bounded outgoing mailbox.
Events retain their relative order. All non-event frames (including earlier
replies, grants and kill frames) are ordering barriers. A head selected for ABI
send stays pinned until success, including across would-block retries; concurrent
insertion cannot change its retry bytes or the frame removed after success.
Closing drops queued frames and fences further admission, but cannot retract an
already-entered ABI call. Grant-epoch disconnect and native off-switch rules above
are unchanged. The 128-frame / 8 MiB limits include the pinned head.

This is mailbox scheduling, not preemption: a slow in-flight event, a would-blocked
head or an earlier non-event barrier can still delay a reply. It does not extend
read deadlines or alter runtime expiry, heard-history authorization or reasoning
fences. `FaceRuntimeWorkerTests` uses gated ABI fixtures and a fake microsecond
clock (two event commits at 600,000 µs each) to exercise ordering deterministically;
wall-clock waits are watchdogs only. These tests are not actual C-ABI runtime,
native media, deadline-success or acoustic qualification.

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

The landed lifecycle repair (PR #136) and native read-expiry completion
(PR #156 / ADR-0029) are integrated. Face-owned reply scheduling is qualified
locally against the original two post-read 600 ms event-commit delays: the reply
arrives within its unchanged deadline, the barge reaches the guest, and the exact
stopped 1200 ms `Hello` projection and all independent replay hashes verify.
Twenty fresh plain production C-ABI runs also pass those assertions. The driver
now drains admitted outgoing traffic under a bounded watchdog before its original
settling interval; it must not close over a queued barge while a prior ABI send
performs durable work. This changes no protocol or guest deadline. The failed
pre-drain experiment is retained, not counted as a passing run.
Current-head native macOS CI and independent source/operational review remain
required before merge; historical native success is not current-head proof.
`FaceEmbeddedTests` remains explicitly an ABI wrapper fixture. `Integration/` is
separate: it links the real release native-engine library without test hooks,
asserts off-main entry at every C symbol, and drives the real scheduled persisted
Face guest through FaceSurfaceHost with injected media. The
`face_embedded_fixture prepare/verify` example provisions only installation JSON,
then reopens the resulting WAL, checks 3 actual guest steps, the emitted Hello world
proposal, journaled barge and exact stopped 1200 ms heard prefix, and independently
reexecutes every guest output hash. The installed production Face manifest is
unchanged and requires the actual emitted `speech.speak_chunk` operation per
`spec/abi.md`; it does not substitute capability-entry names or test-only grants.
The Swift workflow runs the probe after the existing actual-Wasm tests. Local
Linux execution passes; native macOS results and universal-XCFramework packaging
are separate gates. No hardware, acoustics or second speaker graph is involved.

The runtime hosts restricted transactional WASI; the named test
`persisted_face_wasm_emits_to_compiled_adapter_barges_reopens_and_reexecutes`
executes the actual Face Wasm with the compiled Swift adapter, with a real file
history store. It checks the guest's emitted proposal, 1200 ms barge/cancellation,
owner `speech.heard` reads and `runtime.heard` boundary plus reserved heard-state
file containing only `Hello`, not the unheard ` world`, before the next guest step.
WAL reopen and four independent actual-Wasm re-executions yield equal hashes with
no live adapter oracle during replay. Initial seq0 is valid; a replayed old final
cannot reproduce the action. This is aligned simulated-media proof, not native
alignment or C-ABI/acoustic qualification. PR #79 has merged. New history changes
still require their own native CI proof. This draft is not full M2b delivery.

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
