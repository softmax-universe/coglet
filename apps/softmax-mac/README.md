# Softmax for Mac

Menu-bar-first walking skeleton, macOS 14+, Swift 6 strict concurrency.
Bundle ID: `com.softmax.app`; display name: **Softmax**. The app is an
`LSUIElement` agent (no Dock icon). Face uses the Companion vectors in `Design/Face`;
some other shell surfaces remain placeholders pending design/polish integration.

## Build and run

On a Mac, install Xcode with a Swift 6 toolchain, select it with `xcode-select`,
and install [XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`).
From this directory:

```sh
cargo install cbindgen --version 0.29.2 --locked
bash ../../scripts/build-runtime-xcframework.sh
xcodegen generate
xcodebuild -project Softmax.xcodeproj -scheme Softmax \
  -destination 'platform=macOS' -derivedDataPath build build
open build/Build/Products/Debug/Softmax.app
```

Or open the generated project in Xcode, select the **Softmax** scheme and Run.
Do not commit the generated `.xcodeproj`. Local FaceMac resolves from
`../../surfaces/face-mac`; no network package dependency is needed.

The raw Xcode build is unsigned. For normal development use **`bash scripts/build-dev.sh`**
instead: build, sign embedded frameworks, sign the app, then verify its signature.
Stable permission grants and launch-at-login behavior require a consistently signed app.
CI deliberately builds unsigned: its ZIP is not a distributable release.
Developer ID signing, notarization, DMG and Sparkle remain release work. Local
audio capture and global push-to-talk are implemented here. Local runtime hosting
is described below; hub enrollment and speech-provider delivery remain separate work.

## Embedded runtime

`RuntimeHost` runs the real WAMR runtime through the Swift surface SDK's C ABI
transport on a background actor. UI updates stay on MainActor. State, pinned
configuration, surface ledger, WAL and diagnostics are under
`~/Library/Application Support/Softmax/runtime/`. Settings › Coglets lists the
loaded ID, manifest version and state, and provides Restart runtime and Open logs.
The current runtime coordinator supports **one pinned coglet**, not a multi-coglet
registry; startup rejects unsupported operations rather than reporting them loaded.

To explicitly install the model-free speech fixture from the repository root:

```sh
rustup target add wasm32-wasip1
ROOT="$HOME/Library/Application Support/Softmax/runtime"
python3 apps/softmax-mac/tools/install_speech_echo.py --output-dir "$ROOT" --wal "$ROOT/state.wal"
```

The installer refuses replacement of an existing config/WAL. It is a development
fixture with real Wasm compilation/provenance, not signed hub publication. No demo
coglet or arbitrary code is silently installed by app launch. The product reads
trusted operator-owned `binding.json` and `runtime.json`; see `tools/README.md` and
the config schema. Each start opens the existing WAL and never replays failed work.
A new surface ledger is created only for a new private directory; ledger loss or
unclean recovery fails closed. **Restart runtime** is the explicit local operator
reconciliation/reset, then new negotiation and a higher grants epoch. Grants last
24 hours and are not automatically renewed; Restart is required after expiry.

Push-to-talk captures locally and requests speech-recognition permission when an
utterance is first submitted. Recognition requires on-device support and never
falls back to cloud recognition. A finalized local transcript publishes typed
`speech.start` and `speech.end` through `SurfaceSession`; raw PCM never enters the
control protocol. Speech/expression dispatches reach the bounded `RuntimeAction`
sink (`onAction`). The current Face adapter is deliberately an unavailable sink:
it records the semantic action but sends a failed/unavailable terminal outcome,
not fictitious playback success. Chat text sending, hub enrollment, provider/media
routing and physical TTS/acoustic qualification are not delivered here.

**Fault boundary:** guest traps, runtime API failures, deadline/budget failures
and unwinding Rust panics caught at every C ABI entry become UI `error`. Restart
closes all links, drops poisoned service state, reopens the WAL and establishes
fresh surface authority. **Process-fatal abort, SIGSEGV, OOM, C faults and invalid
caller pointers can still terminate the app.** In-process execution is not a
helper-process crash sandbox. Do not compile the product with `panic=abort` or
`ffi-test-hooks`. The latter supplies a real C-entry caught-panic injection only
for native CI; production archives are rebuilt without it and symbol-checked.

`scripts/build-runtime-xcframework.sh` (at repository root) generates the C header
with pinned cbindgen, builds aarch64/x86_64 Apple static libraries, creates one
universal macOS slice and wraps it in an xcframework. Both architectures are
SDK-linked; only the runner architecture is executed. `RuntimeHostTests` executes
the real compiled speech guest, a compiled trap guest, and caught-panic/restart
paths on `macos-latest`. It does not prove x86_64 execution or physical speech.

## Try the shell

- First launch opens **Welcome to Softmax**. Walk through microphone permission,
  Input Monitoring, then optional launch at login. OS requests happen only on
  explicit button presses. Denied permissions do not prevent finishing onboarding.
- The status menu opens the floating Face or Settings, displays the
  (currently offline) hub status, and quits the app. Face does not activate the app
  when shown; it can accept keyboard focus for accessible controls when clicked.
  Settings and onboarding do activate normally.
- Settings has General, Hotkey, Audio, Hub, Coglets, Updates and Privacy tabs.
  General → **Show floating orb** enables an optional always-on-top 68-point orb,
  in addition to the menu bar. It defaults off and persists under `floatingOrbEnabled`.
  Click opens/closes a 408×548 chat panel; right-click opens its Voice tab.
  Dragging snaps to the nearest left/right visible-screen edge with a 16-point inset.
  Monitor removal rehomes it on the main display. Toggle off also hides the conversation.
  Open Chat / Open Voice remain accessible from the menu bar when the orb is off.
  The conversation's small menu clears the draft or hides the orb. Chat sending and
  runtime speech delivery remain explicitly unavailable until runtime integration lands.
  Check for Updates opens the honest Updates placeholder; it does not claim that
  an update check ran.
- Microphone is checked with `AVCaptureDevice`; Input Monitoring with CoreGraphics
  listen-event access APIs. Refresh when returning from System Settings. macOS
  may require quitting and reopening after an Input Monitoring grant. A false
  Input Monitoring preflight is reported as **Not granted**, not an invented denial.
- Launch at login uses `SMAppService.mainApp`. Registration errors and the
  requires-approval state are displayed; there is no private launch-agent shim.
- Onboarding completion is stored under `onboardingComplete` in standard defaults.
  To replay it: quit Softmax, then `defaults delete com.softmax.app onboardingComplete`.

## Tests

```sh
# Portable pure-core tests; also works on Linux with Swift 6 installed:
swift test

# Native build, SoftmaxCoreTests, and SoftmaxUITests.testLaunchAndOpenSettings:
xcodegen generate
xcodebuild test -project Softmax.xcodeproj -scheme Softmax \
  -destination 'platform=macOS' -derivedDataPath build \
  -resultBundlePath TestResults.xcresult CODE_SIGNING_ALLOWED=NO
```

The UI smoke launches the real app, clicks **Open Settings…** from onboarding
and checks all seven tabs. `--ui-testing` keeps onboarding fresh and suppresses
preferences writes; it does not grant or fake OS permissions. It does not click
permission-request or login-registration buttons, so CI never relies on TCC prompts.

`.github/workflows/softmax-mac.yml` runs these tests on `macos-latest` and uploads
`Softmax.app.zip` (preserving the bundle) and the native `.xcresult` diagnostics.
Core tests cover onboarding gates, valid/invalid transitions, interruption,
reconnection, error recovery, and permission mappings including unknown values.

## Layout

- `Sources/SoftmaxCore`: platform-independent value state machine and permission model.
- `Sources/SoftmaxApp`: main-actor SwiftUI/AppKit shell and OS permission adapters.
- `Tests/SoftmaxCoreTests`: XCTest via both SwiftPM and Xcode.
- `Tests/SoftmaxUITests`: one native XCUITest launch/Settings smoke.
- `project.yml`: XcodeGen app, framework and test targets; `Package.swift` exposes
  only the pure core for fast non-UI tests.

FaceMac supplies the small `AudioCapture` interface and a buffering adapter over the
Face lane's `FaceAudioKit` (mono PCM16 at 24 kHz, VoiceProcessingIO/AEC). There is no
second AVAudioEngine implementation. Queue failures and graph/device rebuilds cancel
the utterance instead of silently switching microphones. No connectivity or
speech-provider result is fabricated.

## Push-to-talk and runtime seam

After onboarding, hold **Right Option** in any app. The listen-only CGEventTap does
not swallow shortcuts. A non-activating HUD near the top-right menu bar displays
RMS input level and elapsed time; the status icon becomes a microphone. Release
moves to thinking and emits PCM. Under 250 ms is discarded; 120 s ends the session
without rearming until release. Settings › Hotkey records a key/chord (or standalone
modifier), persists the binding, and offers toggle mode, reset and optional start sound.
A denied/missing Input Monitoring grant opens Privacy; grant there and relaunch if
macOS requires it. Microphone and Input Monitoring settings links remain available.
Capture is local and can be exercised while the hub is offline.

`ShellModel.ptt.speechInput` is a **single-consumer**, bounded async stream of
`SpeechInput.utterance(Utterance)` and `.bargeIn(timestamp:)`. `Utterance.frames` is
mono Float32 PCM with per-frame sample rate and monotonic timestamp (system uptime,
seconds); `startedAt` and `endedAt` use the same timebase. Audio is buffered in memory,
not written to disk. `AudioCapture.stop()` synchronously drains the authoritative
session buffer; its bounded `frames` stream is for live observers only.

The runtime card must consume that stream and call
`finishSpeechInput(operation: utterance.operationID)` on delivery completion/failure.
**Until that integration exists, thinking intentionally means an
utterance is awaiting the runtime, not that transcription was performed.** No speech
provider request is made here. Before Face playback starts, runtime calls
`beginFacePlayback(stop:)` with its synchronous stop closure and retain the returned
operation ID; when playback ends, call `finishFacePlayback(operation: id)`. Stale
completions are ignored and cannot cancel a newer capture. A press during that interval invokes stop before capture
and emits bargeIn. `machine` external state changes also synchronize the controller.
This is the playback seam, not a new TTS implementation.

## Face host and speech output

Open Face from the menu, or launch with `--show-face`. Its nonactivating floating
panel joins all Spaces, remembers its dragged position, and clamps restored bounds
to a visible screen. Compact Face reduces it to an orb; Expand Face restores the
280-point panel. Escape cancels unsent capture and speech, stops the simulated demo,
and hides the panel. While another app has focus, global Escape observation requires
an existing keyboard-observation grant; no new permission is requested by Face.
The visible Hide Face and Stop buttons remain available without that grant.

**Start simulated demo** loops idle/listening/thinking/speaking without capturing or
playing audio. It is explicitly labelled simulated and stops on real capture.
The `FaceDriver` seam takes state and expression 0.1 values; the current spec has no
viseme payload. Reduce Motion removes scale motion and transition interpolation;
semantic mouth/eye changes remain visible. VoiceOver has a state label and native
buttons. Hiding suspends the render timer; expression expiry uses monotonic time.

Runtime integration owns synthesis, grants, deadlines and action outcomes. It
replaces the demo driver and calls `FaceHost.beginSpeech()` to obtain a fresh token,
then `host.speech.enqueue(PCMFrame(...), token:)` for mono Float32 TTS frames and
`host.speech.finish(token:)` at synthesis end. Do not also call the older
`ShellModel.beginFacePlayback` seam for this host. `FacePlaybackSession` joins this
sink to PTT; replacing or stopping output fences old frames/completions. The sink
finishes only after native played-back callbacks, not after synthesis/scheduling.
It bounds queued audio to ten seconds, one second per frame and 512 pending
envelopes; the producer must backpressure rather than drop a failed frame.

`FaceTests` covers each state/expression, invalid inputs/expiry, replacement,
rendered completion and the local PTT stop path with a fake 4 ms audio stop clock.
This is **not** a hardware/acoustic latency measurement. `FaceSnapshotTests` compares
twelve native SwiftUI light/dark renders with reviewed committed PNGs, failing on
missing baselines. CI uploads `Face-native-snapshots`; intentional baseline updates
must come from that macOS renderer and be visually reviewed before committing.
The UI test exercises compact/expand, labelled demo and Escape and attaches a
native panel screenshot to the `.xcresult` bundle.

## Real-Mac acceptance checklist (not proven by CI)

Use a consistently signed installed app on macOS 14+. Record OS/build, input/output
device, binding/mode and observations; do not infer acoustic behavior from fake tests.

- Deny microphone/Input Monitoring separately: no capture; Settings/onboarding links
  open the correct pane. Grant, relaunch if required, and retry. Revoke mid-session:
  capture/HUD stop; no utterance is delivered.
- Hold Right Option in another app: HUD appears without focus theft; meter reacts to
  speech, timer advances, icon changes. Release: HUD disappears, thinking + timestamped
  PCM. Left Option alone must not trigger; hold both Options and release Right first.
- Short tap (<250 ms): discard, no utterance. Hold past120 s: exactly one capped
  utterance, no restart while still down, release is harmless.
- Record a new chord in Settings, relaunch, verify persistence; reset to Right Option.
  Toggle: press/release stays listening; next press stops; autorepeat never toggles.
- Exercise `beginFacePlayback(stop:)` with actual Face playback: press audibly stops it
  and runtime observes bargeIn before utterance. Confirm no late playback resumes.
- Verify voice processing/AEC on speakers and headphones with real acoustic samples;
  check default device changes, sleep/wake, secure-input/tap interruption, and quit
  during recording. Native compile/UI smoke does not prove these hardware behaviors.
- Start sound off/on behaves as selected; no recording indicator remains after cancel.

`PushToTalkTests` inject fake keys, audio and clock; no unit test requests TCC access.


## Development signing and microphone identity

`scripts/sign-dev.sh` adapts Companion's existing-identity approach. It uses
`SOFTMAX_DEV_SIGNING_IDENTITY` (a keychain certificate's 40-character fingerprint),
then the local ignored `.dev-signing-identity` pin, otherwise the sole available
code-signing identity. Multiple/no identities trigger a loud **ad-hoc fallback**:
that fallback cannot promise microphone permission continuity. An unavailable pin
fails closed, rather than silently changing identity. Keep the certificate and its
private key in your keychain; the script never creates or exports one. To deliberately
rotate, set the environment variable to the replacement fingerprint; expect a new grant.
Don't delete the pin during normal rebuilds. Release signing/notarization is separate.

### Manual two-rebuild TCC acceptance (Mac only, NOT verified on CI)

The Linux worker and ephemeral unsigned CI cannot prove persisted macOS TCC grants.
The automated fake-keychain tests prove selection/pinning and nested-signing order,
**not** permission survival. Run on a development Mac with an existing stable identity:

1. `security find-identity -v -p codesigning`; choose your certificate fingerprint,
   then `export SOFTMAX_DEV_SIGNING_IDENTITY=<fingerprint>`.
2. `bash scripts/build-dev.sh`; open the resulting app at the **same path** for every run.
   In onboarding allow Microphone. In Audio confirm `granted`. Quit Softmax completely.
3. Save `codesign -d -r- build/Build/Products/Debug/Softmax.app 2> requirement-1.txt`.
4. Make a harmless source change, `bash scripts/build-dev.sh`, save the requirement
   as `requirement-2.txt`, reopen at the same path. Confirm `granted` with no prompt; quit.
5. Repeat another source change/build/open, save `requirement-3.txt` and compare all
   three designated requirements (`diff`). Confirm `granted` again without prompting.
   Record macOS version, certificate fingerprint, three requirements, and observations
   in the PR. This signing check alone does not prove actual audio capture; use the
   separate push-to-talk real-Mac acceptance checklist above.
6. Denial path: turn Softmax off in System Settings → Privacy & Security → Microphone,
   return to Audio/Voice and refresh. Expect actionable denial copy, a Settings button,
   and no repeated request button. Restricted policy directs the user to their admin.
   Turn the grant back on and refresh (or relaunch as macOS requires).

### Bundle checklist

- Generate project; build app, embedded SoftmaxCore framework and FaceMac dependency.
- Preserve `Contents/MacOS`, `Contents/Frameworks`, Info.plist and resource structure.
- Verify `com.softmax.app`, `LSUIElement`, macOS 14 minimum and
  `NSMicrophoneUsageDescription` in the **built** Info.plist.
- Sign inner frameworks before the app; verify with `codesign --verify --deep --strict`.
- For distributable releases use the release pipeline's signing, entitlements,
  notarization, stapling, DMG and update checks, not these development scripts.
- The design owner selected native SF Pro, not Rubik/Overpass Mono (system mono for
  future code/shortcut labels). No bundled font files or new font license payload.
  This delta retains native symbol/material placeholders; `Design/` assets and
  `docs/design/softmax-app.md` are the design of record and win over these placeholders.
