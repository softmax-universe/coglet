# Softmax for Mac

Menu-bar-first walking skeleton, macOS 14+, Swift 6 strict concurrency.
Bundle ID: `com.softmax.app`; display name: **Softmax**. The app is an
`LSUIElement` agent (no Dock icon). All visuals are placeholders pending the
design/polish lane.

## Build and run

On a Mac, install Xcode with a Swift 6 toolchain, select it with `xcode-select`,
and install [XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`).
From this directory:

```sh
xcodegen generate
xcodebuild -project Softmax.xcodeproj -scheme Softmax \
  -destination 'platform=macOS' -derivedDataPath build build
open build/Build/Products/Debug/Softmax.app
```

Or open the generated project in Xcode, select the **Softmax** scheme and Run.
Do not commit the generated `.xcodeproj`. Local FaceMac resolves from
`../../surfaces/face-mac`; no network package dependency is needed.

The unsigned development build is for local use only. Stable permission grants
and launch-at-login behavior require a consistently signed, installed app.
CI deliberately builds unsigned: its ZIP is not a distributable release.
Developer ID signing, notarization, DMG and Sparkle remain release work. Local
audio capture and global push-to-talk are implemented here. Runtime hosting, hub
enrollment and speech-provider delivery remain separate integration work.

## Try the shell

- First launch opens **Welcome to Softmax**. Walk through microphone permission,
  Input Monitoring, then optional launch at login. OS requests happen only on
  explicit button presses. Denied permissions do not prevent finishing onboarding.
- The status menu opens the floating Face placeholder or Settings, displays the
  (currently offline) hub status, and quits the app. The Face panel does not take
  keyboard focus. Settings and onboarding do activate normally.
- Settings has General, Hotkey, Audio, Hub, Coglets, Updates and Privacy tabs.
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

The runtime card must consume that stream and call `finishSpeechInput()` on delivery
completion/failure. **Until that integration exists, thinking intentionally means an
utterance is awaiting the runtime, not that transcription was performed.** No speech
provider request is made here. Before Face playback starts, runtime calls
`beginFacePlayback(stop:)` with its synchronous stop closure; when playback ends,
call `finishFacePlayback()`. A press during that interval invokes stop before capture
and emits bargeIn. `machine` external state changes also synchronize the controller.
This is the playback seam, not a new TTS implementation.

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
