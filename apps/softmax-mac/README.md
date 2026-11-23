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

The raw Xcode build is unsigned. For normal development use **`bash scripts/build-dev.sh`**
instead: build, sign embedded frameworks, sign the app, then verify its signature.
Stable permission grants and launch-at-login behavior require a consistently signed app.
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
  General → **Show floating orb** enables an optional always-on-top 68-point orb,
  in addition to the menu bar. It defaults off and persists under `floatingOrbEnabled`.
  Click opens/closes a 408×548 chat panel; right-click opens its Voice tab.
  Dragging snaps to the nearest left/right visible-screen edge with a 16-point inset.
  Monitor removal rehomes it on the main display. Toggle off also hides the conversation.
  Open Chat / Open Voice remain accessible from the menu bar when the orb is off.
  The conversation's small menu clears the draft or hides the orb. Chat sending and
  voice capture remain explicitly unavailable until runtime/audio integration lands.
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
   in the PR. Never claim this proves actual audio capture (not wired yet).
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
