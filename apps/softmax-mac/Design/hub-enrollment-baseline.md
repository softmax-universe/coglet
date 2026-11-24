# Baseline: ROUTER enrollment

Kind: extension
Request: "Enrollment: done from onboarding or Settings › Hub, using the spec's enrollment flow. Support a pairing code or QR from the hub, plus a manual hub address for developers. Show the enrolled hub's name and status."
Delta: Replace the existing enrollment placeholders with one shared local pairing form and connection summary, preserving surrounding settings and onboarding composition.

This is a Configure surface, not a redesign. Existing identifiers stay unchanged; new user-facing copy calls the hub a ROUTER.

## Surface: macOS Settings Hub
Mount: apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:106 SettingsView hub branch inside the existing DesignGroup
Siblings: Filter apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:38 — native field, 25pt standard/35pt largest height, no icon, 180pt padded sidebar; section buttons apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:57 — borderless, 6pt padding, radius6, accent14% selection fill, no icons; keyboard shortcuts apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:68 — native button, no icon; heading apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:73 — 20pt semibold; group apps/softmax-mac/Sources/SoftmaxApp/Design.swift:77 — full width, 16pt inset, 12pt gaps, radius10, surface fill, 1pt border/2pt increased contrast.
Chrome: Fixed180pt sidebar, search, seven sections, shortcut footer, divider, scrollable content with24pt padding and section heading; minimum760x540.
Absent: No tab strip, no additional navigation, no dashboard tiles, no floating enrollment window.
Tokens: apps/softmax-mac/Design/DesignTokens.swift:4 — existing generated body14/caption11/title20/display28; spacing4/8/12/16/24/32; Light and Dark background/surface/ink/border/accent; native button geometry retained.
Render: apps/softmax-mac/Tests/DesignSnapshots/light-settings-hub.png | captured unknown | commit d7fd9bfe66416d6a7c1ac79b904eb16d9a8fc9be | stale: snapshot committed2026-11-23 before subsequent Windows.swift changes; inspected layout matches hub placeholder, source is authoritative for current siblings.
Render: apps/softmax-mac/Tests/DesignSnapshots/dark-settings-hub.png | captured unknown | commit d7fd9bfe66416d6a7c1ac79b904eb16d9a8fc9be | stale: snapshot predates current Windows.swift; retain source geometry and generated dark tokens.
Keep: Sidebar/search/filter keyboard behavior, section order and title, shortcut footer, DesignGroup border/padding and theme/text-scale system; use progressive disclosure for manual developer input within existing content group.
Forbid: class="hub-dashboard", class="new-sidebar"

## Surface: macOS Onboarding final step
Mount: apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:192 OnboardingView final branch inside existing DesignGroup
Siblings: Illustration apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:167 — existing100pt-high asset, scaledToFit, no new icon; login controls apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:195 — status plus three native text buttons, vertical8pt spacing, no icons or custom fills; voice status apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:196 — existing14pt text; footer apps/softmax-mac/Sources/SoftmaxApp/Windows.swift:202 — native Open Settings/Back/Finish buttons, spacer and default-action shortcut.
Chrome: 3-of-3 progress, 28pt title, existing illustration, scrollable group, fixed navigation footer,24pt outer inset, minimum620x600.
Absent: No sidebar, no new wizard stages, no tab bar, no new header navigation.
Tokens: apps/softmax-mac/Design/DesignTokens.swift:4 — same macOS theme and typography as Settings; DesignGroup radius10/surface fill/16pt inset/12pt content spacing.
Render: apps/softmax-mac/Tests/DesignSnapshots/light-onboarding-2.png | captured unknown | commit d7fd9bfe66416d6a7c1ac79b904eb16d9a8fc9be | stale: snapshot committed2026-11-23 before current Windows.swift; inspected progress/title/illustration/login/footer, source controls take precedence.
Render: apps/softmax-mac/Tests/DesignSnapshots/dark-onboarding-2.png | captured unknown | commit d7fd9bfe66416d6a7c1ac79b904eb16d9a8fc9be | stale: snapshot predates current Windows.swift; preserve source structure and dark tokens.
Keep: Progress, illustration, title typography, optional completion without enrollment, login item controls, voice-readiness copy, Open Settings/Back/Finish footer, current theme/large-text support; only replace the two placeholder text rows with shared enrollment controls.
Forbid: class="onboarding-sidebar", class="fourth-stage"

Capture times are not retained with these PNGs; the cited commit is publication provenance, not a claimed capture timestamp. Four renders listed above were opened and inspected during continuation3; previous runs additionally inspected all eight Settings variants. Native post-change large-text/high-contrast comparison remains required.

## Verification
Status: unverified: baseline inspected; no enrollment UI changes or native post-change render comparison yet
