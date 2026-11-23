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
  `XCUIApplication.performAccessibilityAudit()` for all three onboarding chapters
  and all seven Settings sections. All product controls and all contrast findings
  fail closed. The handler logs and excludes only two OS chrome findings observed
  in native run 45116636568: missing description on the disabled empty application
  Touch Bar, and parent/child mismatch inside the system `_XCUI:ZoomWindow`
  button's disabled 14×14 group. No product element, missing action, contrast,
  clipping or dynamic-type failure is exempt. Review the handler with native CI;
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

## Review log (not baseline approval)

Run `45117496532` at `a2d19b0` produced all 232 manifest-matched native PNGs.
All ten contact sheets were visually inspected, plus full-size light/dark General
and the labelled General native/HTML comparison. No blank surface was found.
The palette and overall Companion hierarchy are present, but this is **not**
acceptance of the matrix: onboarding Voice content extends below its viewport,
General needs scrolling at largest text, explicit title/caption fonts did not
scale with body copy, and the live accessibility audit still failed contrast.
The native fixture renderer sets both AppKit appearance and SwiftUI scheme;
its cached-display rasters do not establish live-window audit contrast.

The next CI exports `Softmax-live-window-audit-evidence` using xcresulttool,
including pre-audit screenshots, to diagnose actual compositor pixels. Contrast
findings remain failures; no new audit exemption was introduced. Local surface
ink pairing is an experiment pending native confirmation. Explicit token fonts
now follow the text-size preference, and selected Settings rows use the designed
accent with an increased-contrast outline; these changes need new native review.

## Live audit diagnosis and next revision (not acceptance)

Run `45118684325` at `b3e4db5` still failed eight product contrast findings.
Its exported audit crops, not just the fixture renderer, contain actual ink
`#17231D` on white (16.22:1) and danger `#A12C35` on white (7.17:1).
The offscreen Input Monitoring paragraph's audit crop instead contains the
footer, so changing its ink cannot repair that sample. Adding foregroundColor
at the group boundary did not change the failures.

Run `45119838830` at `6811a4d` isolated nine identical Appearance labels, with no
product theme or nested stateful controls. Exactly `token-13-regular`,
`black-13-regular` and `token-13-leaf-background` failed. The 14/15/17 pt regular,
13 pt medium/semibold, and native semantic body rows passed. This establishes a
font/raster-sensitive audit failure, not a bad token colour or opaque-surface
inheritance. It does not establish the audit engine's internal algorithm.

The next revision therefore raises the body token to 14 pt, keeps a native
body-token/diagnostic-red regression, and runs unchanged fail-closed product
audits in **both light and dark**. Onboarding is 620×600 with smaller voice art
and horizontal permission actions (vertical fallback at large text), so its
ordinary permission copy is no longer deliberately below the fold. Largest-text
scrolling and Back round trips have explicit XCUITests. No contrast exemption
was added. This revision still needs native compilation, real rasters and audit
results; it is not baseline approval.

The matrix now requires 360 captures, extending 1.6× coverage beyond Settings to
onboarding, HUD, menu, Face, About, and the global command palette's default,
filtered and empty states. The HUD grows with its text preference instead of
retaining an 88 pt clip. Cmd-K now searches actual app destinations and Settings
synonyms; / or typing on Settings filters locally, with literal match emphasis.
No unimplemented coglet or hub object is invented for search results. Native
keyboard tests exercise arrows, activation, dismissal and focus return.

## Keyboard remediation and deterministic capture

Native run `45124822512` reproduces both review findings with real key input:
selected command rows leave the viewport, and focused Settings search consumes
arrows. Run `45126403527` after the production fixes passes all 39 unit and 9 UI
tests, including both regressions; `45128197127` repeats those passes and the
light/dark accessibility audits. These runs are not final snapshot acceptance.

Run `45128197127` exposed 15 remaining selected-query reference highlights after
field-editor selection normalization. Every changed pixel lies within the query
text rectangle (x26–215, y24–51); all 15 old/new crops were visually inspected
before updating only those references. One additional Settings Hub capture had
a 10 pt taller empty search field; its reference is deliberately unchanged.
The renderer now requires three consecutive identical PNG frames after at least
200 ms of run-loop/layout settling, with a two-second fail-closed deadline per
fixture. This replaces the single 40 ms delay, not the pixel comparison gate.
Run `45129426172` compiled this settling harness, rendered all 360 fixtures and
passed all 39 unit plus 9 UI tests. Exactly one PNG differs from its reference:
`light-commands`, with changed pixels confined to (415,83)–(440,116). Full-size
old/new inspection shows only the selected row's right edge is 17 pt narrower;
all other 359 images are byte-pixel identical, including the previously unstable
Hub field. This is consistent with a legacy scroller gutter, not changed text.
The capture harness now explicitly sets descendant NSScrollViews to overlay
scrollers and logs each legacy-style conversion. This fixture-only hypothesis
still requires native execution and strict 360-state comparison; no reference,
production preference, comparator tolerance or accessibility waiver changed.

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
