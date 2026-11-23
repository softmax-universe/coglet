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
Developer ID signing, notarization, DMG, Sparkle, runtime hosting, hub enrollment,
audio capture and the global hotkey are later integration/release work, not
implemented by this skeleton. The hotkey is currently a hint, not a live binding.

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

The FaceMac package is linked but its placeholder API is intentionally not
assumed. The shell stays offline until a real hub adapter is added: no fabricated
connectivity, runtime, recording or update result is presented.
