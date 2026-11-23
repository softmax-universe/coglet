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

The FaceMac package is linked but its placeholder API is intentionally not
assumed. The shell stays offline until a real hub adapter is added: no fabricated
connectivity, runtime, recording or update result is presented.

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
- Brand assets/fonts remain owned by the design card. This delta adds no copied fonts
  or logo and therefore no new font license payload; if Rubik/Overpass Mono is adopted,
  bundle its OFL text alongside the font files. Native symbol/material placeholders stay
  replaceable by the design of record.
