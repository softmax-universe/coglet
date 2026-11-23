# Native Companion presentation verification

The app compiles `Design/DesignTokens.swift` directly. Change JSON tokens and run
`uv run --with cairosvg --with pillow Design/generate-assets.py`; do not maintain
another Swift token copy. Native PDF assets are compiled with
`uv run --with cairosvg scripts/compile-design.py` and committed in the asset catalog.
`AppIcon` is the selected Companion icon. Menu images have template intent, not
baked light/dark colours. Onboarding artwork keeps its original colours.

## Native test path

`Softmax Mac` CI on `macos-latest` builds the real app and runs XCUITest:
- existing Face/orb/window navigation smoke tests;
- `testOnboardingAndEverySettingsSectionAccessibilityAudit`, using
  `XCUIApplication.performAccessibilityAudit()` with no issue suppression for all
  three onboarding chapters and all seven Settings sections;
- keyboard search, synonym, Return and Escape round trip.

The built app's isolated `--ui-testing --design-snapshots OUTPUT` entry point
renders production NSHostingViews, not HTML, and validates compiled template
resources and activity/error/offline/update precedence. It does not start capture,
connect a hub or enable Sparkle. Fixture input levels are explicitly test data.
No background Face demo or menu animation runs during captures.

The explicit state matrix in `scripts/check-design-snapshots.py` must agree with
`manifest.json`; missing references or removed coverage fail closed. It covers
light/dark, standard/increased contrast, onboarding chapters and all permission
and login-item statuses, every Settings tab and its largest-text equivalent,
all HUD/menu states, template glyphs, Face states/expressions, and About/licences.
Largest text uses the same explicit 1.6× text-size preference offered by the Mac
app; it does not pretend that macOS implements iOS Dynamic Type.

References belong in `Tests/DesignSnapshots/`. The first capture is deliberately
red until its images are inspected and committed. CI never records new goldens
or accepts missing files. Comparison permits channel noise ≤10, with at most
0.2% other changed pixels; blank/uniform captures fail independently. Mutation
unit tests reject missing coverage and a changed raster.

`Softmax-native-design-and-comparisons` contains native PNGs, machine-readable
report and labelled native/HTML side-by-sides. HTML screenshots retain their
whole desktop context and are review context only, never pixel-equality goldens.
Attach the reviewed side-by-sides to the PR and cite the exact successful run.

## Intentional differences from the design study

- Keep Right Option and near-menu-bar HUD from the more-specific PTT acceptance.
- Hub enrollment and runtime lifecycle/grant integration remain unavailable;
  there is no fictitious connection, running coglet, permission grant or version.
- The existing optional orb/chat/voice shell and operation fencing are retained.
- Native permission controls show actual macOS statuses; no synthetic success.
- About reads bundle version/build/channel and discloses the absent runtime;
  bundled notices contain Sparkle 2.10.1's complete MIT and third-party texts.

A native raster comparison is not proof of acoustic performance, a real-account
TCC allow/deny cycle, signed updates or notarization. Those remain with their
existing owners; do not claim them from this visual suite.
