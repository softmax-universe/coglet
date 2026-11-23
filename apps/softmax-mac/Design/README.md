# Softmax Mac design package

This is the **Companion** product design, not the signed application. Start with [the design of record](../../../docs/design/softmax-app.md) and [implementation plan](../../../docs/design/softmax-app-implementation.md).

Open [the clickable prototype](mockups/softmax-mac.html) directly in a browser. It has three composition directions, light/dark appearance, every surface state, Settings scenarios, keyboard search and a local Space-to-talk simulation. All content is sample data; no permissions, audio, hub requests or native changes occur.

## Files

- `AppIcon.svg`: editable 1024 master; `AppIcon.appiconset/`: all ten macOS slots.
- `MenuBar/`: 28 static templates (seven states × two point sizes × two scales) and 32 animation frame SVGs. Black alpha-only artwork; `animations.json` gives timing.
- `Face/`: seven layered poses and a presentation-only channel/motion specification. Expression protocol remains unspecified; do not treat these JSON keys as capability payloads.
- `DMG/`: 660×440 SVG and 1×/2× PNGs; app/Applications positions are specified in the design record.
- `Onboarding/`: three editable source illustrations.
- `DesignTokens.json`: color, type, space, radius and timing source. `DesignTokens.swift`: generated SwiftUI values.
- `concepts/`: eight source icon concepts and their small-size contact sheet.
- `mockups/screenshots/`: rendered state captures and template-image proofs; [screenshot gallery](mockups/screenshots.html).
- `mockups/verification.json`: enumerated browser checks, counts and native-boundary caveats in the design record.

## Reproduce

From repository root, with Python 3, [uv](https://docs.astral.sh/uv/) and Cairo installed:

```sh
uv run --with cairosvg==2.9.1 --with pillow==13.0.0 python apps/softmax-mac/Design/generate-assets.py
bash scripts/check-assets.sh
uv run --with playwright==1.65.2 --with pillow==13.0.0 python -m playwright install chromium
uv run --with playwright==1.65.2 --with pillow==13.0.0 python apps/softmax-mac/Design/verify-mockups.py
uv run --with pillow==13.0.0 python apps/softmax-mac/Design/build-gallery.py
```

The generator rebuilds all exports from its vector definitions and the JSON token definition; preserve edits there before regenerating. The asset checker uses only the Python standard library, validates dimensions and PNG CRCs, the complete macOS slot set, SVG monochrome constraints, animation frame presence/variation, layered Face sources, Swift-token correspondence and text contrast. The `docs` CI job runs it on pushes and PRs. Browser checks launch their own Chromium and use page-targeted input only; no desktop keyboard injection.

Screenshot parameters: `?view=menu&state=listening&theme=dark&dir=companion&still=1`. `view` and `state` correspond to the visible selectors. Settings adds `scenario=denied` or another available scenario. `still=1` freezes menu frame playback. The capture harness asserts every modeled state in both themes was visited; it checks narrow overview layouts and real typing/shortcuts, not merely synthetic events.

## Native integration boundary

SVGs are editable sources, not a promise that `NSImage` can directly decode SVG. Compile template vectors to PDF or raster resources, set `isTemplate=true`, and test in native light/dark menu bars. The HTML proofs show the exact alpha masks on two backgrounds but are **not AppKit screenshots**. `AppIcon.appiconset` validates structurally on Linux; Xcode `actool`, Swift compilation, Finder DMG placement, VoiceOver, TCC and real capture/playback require macOS CI/device testing. No signing/notarization/update-binary claim is made by this design package.
