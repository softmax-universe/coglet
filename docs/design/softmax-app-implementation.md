# Softmax Mac implementation plan

This companion to [the design record](softmax-app.md) defines a future native implementation; these tasks are **not implemented by the design package**. No expression wire format is introduced here.

## Ownership and dependencies

1. **App shell / distribution owner**: `apps/softmax-mac/Sources/SoftmaxApp.swift`, `AppState.swift`, `MenuBarController.swift`, `Resources/Assets.xcassets/`; use SwiftUI + AppKit, macOS 14+, menu-bar accessory activation policy, opt-in Dock icon. Integrate the existing local runtime library/service boundary, never create a new engine. Depends on runtime C API and Swift surface SDK. Import generated `Design/DesignTokens.swift` into the app target; compile `AppIcon.appiconset` using `actool`. Convert menu SVG sources to PDF or approved PNG resources before bundling, preserving vector alpha and optical dimensions. Tests: `MenuStateReducerTests`, `TemplateResourceTests`, `RuntimeShutdownTests`, `AppIconCompileTests`.
2. **Face/audio owner**: `FacePanel.swift`, `PushToTalkController.swift`, `AudioSettingsView.swift`; consumes `surfaces/face-mac/` audio kit and `coglets/face/`, no duplicate capture pipeline. Implement nonactivating panel, holding/release and optional toggle, Esc cancel, local playback interruption, device-loss cancellation. Parameter mapping to the expression capability waits on the expression owner's ADR/spec. Tests: `PushToTalkTransitionTests`, `KeyUpLossCancellationTests`, `AudioDeviceLossTests`, `CancelledResponseFenceTests`, `ReduceMotionFaceTests`. Real acoustic tests remain with the audio kit owner.
3. **Settings/onboarding owner**: `Settings/` (General, Hotkey, Audio, HubAccount, Coglets, Updates, Privacy), `Onboarding/`, `ShortcutRouter.swift`. Adopt typed state, not strings from the HTML demo. Choose the global-hotkey API and prove permission requirements before writing permission copy; request only what it needs. Use ServiceManagement for opt-in login item. Persist choices only after validation, avoid credential logs. Tests: `PermissionRecoveryTests`, `ShortcutConflictTests`, `LoginItemExternalDisableTests`, `SettingsSearchTests`, `KeyboardRoundTripTests`, `EnrollmentIdentityConfirmationTests`.
4. **Release owner**: `Packaging/`, entitlements, signed Developer ID build, notarization, DMG and Sparkle. Pin signed feed configuration. DMG content area 660×440 with app/Applications positions as designed. Test downloaded notarized DMG on a clean supported macOS account, Gatekeeper approval and upgrade path. Tests: `DMGLayoutTests`, `SignedUpdateVerificationTests`, `UpdateDefersDuringAudioTests`. Never ship placeholder feed URLs or sample version strings.

## Shared state/data flow

Audio surface publishes actual capture/playback state. Hub link publishes connection and identity facts. Runtime publishes per-coglet lifecycle and grants. Updater publishes check/download/verify/install state. The application reducer combines these into menu icon precedence without overwriting underlying facts. Face animation reads a presentation projection of those facts; it never decides whether the microphone is open. Settings and onboarding dispatch validated intents to the same controllers; their state cannot drift from the menu extra.

Keep last accepted preferences separate from drafts and errors. A microphone test is an explicit capture session with a Stop affordance. Disconnect, export and deletion use confirmable intents; export redacts secrets before a preview is shown. App termination stops capture and playback before shutting down its local runtime process.

## Native acceptance matrix

- **Assets**: run `scripts/check-assets.sh`, compile iconset in Xcode, load every template into `NSImage` with `isTemplate=true`, capture seven states on light/dark native menu bars at 1×/2×. Animated listening and speaking remain distinguishable; Reduce Motion is static.
- **Permissions**: clean-account allow, deny, revoke and return-from-System-Settings flows for microphone and the qualified hotkey API; denial must leave visible Talk/Settings available. Login-item opt-in remains independent.
- **PTT**: hold/release, repeat, toggle, missed key-up, lost hub, removed device, Escape while held, Escape during thinking, stale response after cancellation, barge-in during speaking. Exactly one send on release; none on cancellation.
- **Navigation**: every surface by mouse and keyboard, searchable list counts/highlights/empty state, palette synonyms, Arrow/Return, Back, focus restoration after modal dismissal, no hotkey capture while recording a shortcut.
- **Accessibility**: VoiceOver roles/labels and live announcements, full keyboard access, dark/light and increased contrast, Reduce Motion/Transparency, large text; no indicator depends only on color.
- **Privacy**: no capture before explicit permission and activation; no raw audio/transcript in default logs; verify invitation identity; grant changes are explicit; destructive actions never fire on initial click.
- **Distribution**: signed/notarized DMG, Finder alignment on Retina/non-Retina, Sparkle rejects invalid signatures, does not relaunch during capture/playback, restores preferences after update.

The HTML/browser verification report establishes only the design prototype and asset contract. It must not be substituted for these native tests or the separate runtime/audio conformance suites.
