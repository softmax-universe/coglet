# Softmax for Mac — product and visual design

**Design of record · Companion direction · macOS 14+ · design package, not an implemented app.**

Read alongside [the build plan](../plan.md) and [the runtime design](coglet-runtime.md). This document changes no protocol. The current main branch had no `apps/softmax-mac/` implementation when this study began. All sample account, version, coglet and connection content in the prototype is labelled; none is live telemetry.

[Clickable prototype](../../apps/softmax-mac/Design/mockups/softmax-mac.html) · [assets and build recipe](../../apps/softmax-mac/Design/README.md) · [implementation plan](softmax-app-implementation.md).

## Audience and jobs

For a person installing Softmax on their own Mac, not a runtime developer. They want to speak to Face without switching apps, know when audio is live, connect a trusted hub, and know which coglets run locally. Secondary jobs: repair a connection, select audio devices, inspect grants, and update safely.

The app hosts one local coglet runtime and the Face audio surface; Face is a coglet, not a second runtime. Hub connection, local-runtime health, microphone capture and speech playback are independent facts. A disconnected hub does not mean local coglets stopped. No hub-only action is presented as successful while offline.

## Composition decision

Primary surface: **Command / Inspect**. A small transient Face and a bottom-center talk HUD accompany the current app. Settings is a secondary Configure surface. No dashboard or marketing hero.

Three directions are selectable across the prototype's surfaces:

- **Rail / conservative:** persistent utility column. Familiar control concentration, but expensive desktop occupancy.
- **Companion / selected:** transient menu extra plus a separate 280 pt Face panel and compact HUD. Recommended because menu-bar-first should not mean workspace-first.
- **Stage / divergent:** a centered 340 pt Face. More expressive, but too interruptive for everyday utility use.

Eight flat icon concepts are preserved in `Design/concepts/`. Companion's two eyes and short mouth survive at small sizes, share a silhouette with Face, and avoid letters or model branding. The 1024 SVG has a macOS-style inset rounded tile on transparent corners (not an iOS full-bleed icon). Near-black tile, mint-jade Face, no glow or glossy gradient. Light/dark menu templates use the same alpha silhouette, not recolored app icons.

## Surfaces and states

| Surface | Geometry / purpose | States and recovery |
|---|---|---|
| Menu bar extra | 322 pt, anchored under the status item; status, Show Face, local runtime, Hub, Coglets, Settings, Updates, About, Quit | Idle; listening; thinking; speaking; offline/no hub → Connect/Retry; update available → Updates; error → relevant Settings. Never hide Settings or Quit behind an error. Dismiss outside or Escape. |
| Face overlay | 280 pt floating utility panel, lower-right of the active screen's visible frame; draggable; no screen takeover | Idle; listening; thinking; speaking with Stop; happy; concerned; sleeping/paused; offline with Reconnect; error with Audio/diagnostics action. Closing hides Face, not the runtime. Capture does not silently continue when hiding via Escape. |
| PTT HUD | ~300 × 72 pt, bottom-center above Dock and safe area, does not take keyboard focus | Idle hint; listening with release instruction; thinking; speaking; cancelled/nothing sent; microphone denied; offline; error. Never animate an invented level: native bars use local capture/playback level. Demo bars are explicitly simulated. |
| Settings | 760 pt window, 180 pt sidebar; searchable, grouped rows | Sections below. Valid changes save immediately; input requiring validation saves only after validation. Destructive operations require a separate confirmation; errors preserve the previous setting. |
| First run | 620 pt, three chapters: Welcome; Voice & shortcut (permission substeps); Hub & ready | Welcome; microphone explanatory prompt/granted/denied; hotkey permission/granted/denied; optional login item enabled/disabled/denied; hub disconnected/connecting/verification failure/connected; ready. Back retains choices; Not now skips optional work. Permission substeps are progressive disclosure inside the chapters, not a long marketing tour. |
| About | 420 pt, app icon/name/version/build/channel, support, acknowledgements | Default; licenses/attributions. Runtime version and app version are separate in the real build. The demo's version is a sample. Copy version information is accessible; no fabricated release number in production. |

### Settings inventory

| Section | Controls | Required states |
|---|---|---|
| General | Start at login (default off), Show Dock icon (default off), appearance System/Light/Dark, keep Face visible | Saved; login item denied or disabled externally → link to macOS General → Login Items. Honor an external disable. |
| Hotkey | Recorder, hold/toggle mode, permission status, restore default | Valid; recording; conflicting/reserved shortcut; denied global access; unavailable device/layout. Default proposal ⌥Space, subject to conflict detection; never seize another shortcut. In-window Talk remains usable. |
| Audio | Input/output device, explicit microphone test/stop, sound cues | Ready; testing; denied; missing device; disconnected while recording. Stop capture on input loss; do not hot-switch to another microphone without notice. |
| Hub / Account | Invitation/address, verified node identity, account identity, Connect/Cancel/Disconnect | No hub; connecting; identity confirmation; connected; offline retry; authentication or enrollment denied. Credentials are never echoed in diagnostics. Disconnect confirms effects without claiming all local coglets stop. |
| Coglets | Type-ahead list; name, actual lifecycle, granted capabilities; inspect detail; pause/resume as policy allows | Loading; empty; running/paused; crashed/refused grants; unavailable hub. Do not infer running from registration. Inspect shows capability grant provenance; app cannot grant itself hub authority. |
| Updates | Current app version/channel, automatic checks, Check now, install/relaunch | Checking; up to date; available; downloading/verified/installing; error; deferred while capture/playback active. Sparkle owns verification and installation; unsigned or invalid updates fail closed. |
| Privacy | Microphone and shortcut permission status, per-coglet grants, redacted diagnostics export, local-data deletion | Granted/denied/revoked; export preview/confirmation/error; deletion confirmation. No delete on first click. Never include invitations, raw transcripts or audio in default diagnostics. |

The demo's State selector covers the surface states; Settings additionally exposes a Scenario selector for its failure/loading/confirmation examples. Some native integrations are explanatory preview actions rather than real dialogs. The prototype never captures audio, requests permissions, modifies login items, installs updates or connects a hub.

## Menu bar state machine

Activity is a single enum: `idle → listening → thinking → speaking → idle`. Hold starts capture only after permissions and readiness checks; release closes capture and submits once. Toggle's first activation starts and second submits. Escape cancels capture without submitting, cancels the pending response, or stops playback. Barge-in stops/ducks local playback immediately before starting a new capture. A stale response must not restart cancelled speech.

Orthogonal facts: `hubConnection`, `runtimeHealth`, `permissionState`, `updateAvailable`. Resolve the displayed template in this order:

1. Active listening (privacy-relevant) wins; a critical input failure stops capture immediately and becomes error.
2. Speaking, then thinking.
3. Actionable error (runtime failed, missing microphone, permission revoked).
4. Offline/no configured hub.
5. Update available.
6. Idle.

Do not cover a live microphone indicator with an update badge. The opened menu always shows the orthogonal hub, local runtime and update facts even when another state wins the small icon. Offline is a diagonal cut; thinking is three dots; error is an exclamation; update is an upper-right dot; listening is three input bars; speaking is eyes plus opening mouth; idle is eyes. Tooltip and VoiceOver label carry the full phrase; color is never the only signal.

| Input/event | State/result |
|---|---|
| Hotkey down while ready | Show HUD immediately, request capture, then listening only once capture starts. If denied, show permission recovery instead. |
| Hotkey up after capture | Stop capture, send once, thinking; suppress key-repeat. |
| Toggle activation | Alternate listening and submit; obvious Stop/Cancel buttons remain reachable. |
| Escape / explicit Cancel | Stop capture/playback; discard unsent audio; invalidate pending response; show “Cancelled · nothing sent”. |
| Response starts/ends | Speaking while actual playback is active; return to idle on completion. |
| Hub lost during capture | Stop, retain no implicit queued send; show offline and explain retry. Local coglets retain their actual independent state. |
| Audio permission revoked/device removed | Stop capture; error and focused recovery action. |
| App resigns active while PTT is held | Global native key monitoring continues to track key-up; if tracking is lost, cancel rather than leaving microphone open. Demo blur always cancels. |
| Capture stalled / key-up lost | Surface watchdog cancels and reports failure; no infinite listening. Native timeout configured by audio owner, not guessed by animation. |
| Update event during conversation | Queue availability fact; never interrupt audio; display after active state ends. |

Static templates: 16 and 18 pt, each 1×/2×, monochrome with transparent background. `MenuBar/animations.json` declares four frames at 160 ms for listening and speaking. Set `NSImage.isTemplate = true`; SVG sources must be rasterized or converted to vector PDF at asset compilation because AppKit cannot be assumed to decode raw SVG. Use 18 pt for the normal status item, 16 only for compact presentation. Never scale the 1024 app icon down to the menu bar. Reduce Motion freezes on the explicit static state; no constant idle animation.

## Voice and permission onboarding

- **Microphone:** show purpose before the system prompt; request only after Allow. Denial: “Microphone access is off.” Offer System Settings → Privacy & Security → Microphone → Softmax, Retry status check, or Not now. No repeated permission-loop prompts. Re-check after returning to the app.
- **Global shortcut:** implementation must choose an API first. A non-intercepting registered shortcut may need no Input Monitoring or Accessibility permission; hold/release observation can need Input Monitoring, an intercepting event tap can need Accessibility. Request only the actual API-required permission, not both as a ritual. Explain it is for the global shortcut and that typed text is not retained. Denial keeps the visible Talk button and local settings usable. The prototype explicitly models both denied and review states; it is not evidence of API qualification.
- **Login item:** separate opt-in, default off, using macOS ServiceManagement. Do not call it an audio permission. Show enabled/pending approval/disabled externally/registration error. Link to General → Login Items. Do not repeatedly re-register after external disable.
- **Hub:** paste a trusted enrollment invitation, show verified node/account identity, then confirm. Invalid/expired invitation preserves non-secret input only as appropriate and offers replacement. Connecting has Cancel. No connection is implied merely because a field is filled.
- **Ready:** rehearse hold/release with a harmless utterance and show the menu bar location. Users who skipped microphone or hub finish with an honest “Voice not set up” status and Settings path, never a false ready promise.

## Face visual language

`Design/Face/*.svg` provides independently named `head`, `eyes`, `mouth`, and optional `expression` layers on a 1024 coordinate canvas. No external texture or font dependency. Idle: rounded jade head, two vertical dark eyes, short mouth. Listening: attentive open eyes and very small head scale; thinking: short horizontal eyes; speaking: mouth opening driven by actual playback envelope; happy: upward mouth arc; concerned: asymmetric inward brows; sleeping: closed eyes, explicit Paused label outside the vector.

`Face/motion.json` is **presentation-only**: blink 0…1 over 120 ms, gaze ±18/±12 master units, mouthOpen 0…1 mapped to 16…88 units with 70 ms smoothing, listening scale 1…1.025 over 1100 ms, expression blend 180 ms. Clamp inputs; unknown expression falls back to neutral, never silently implies listening. The expression spec currently contains only a placeholder. These are local renderer assumptions, not capability names or payloads. Mapping semantic expressions to a wire capability requires an independently reviewed ADR from the expression owner before implementation; this design changes no spec.

## Keyboard, sound and motion

| Shortcut | Behavior |
|---|---|
| ⌥Space (proposed, configurable) | Hold to capture; release to send. Toggle optional. Global only after permission/API qualification. Demo uses unmodified Space in its owned page. |
| Esc | Cancel capture without sending; stop speech/pending response; dismiss transient surface; clear filter before closing. |
| ⌘K | Search all surfaces, settings by synonyms, coglets and commands. ↑↓ selects, Return opens, Escape dismisses and restores focus. |
| ⌘, | Open Settings; preserve section. |
| / or typing on a list | Focus current type-ahead filter; highlight literal matches, include synonym matches, show accurate filtered count and empty state. |
| ↑↓ / Return | Select and open visible item; do not use an unfiltered index. |
| ? | Shortcut sheet while not editing. |
| ⌘Q | Quit via confirmation if active capture/response would be lost; surface stops audio before runtime shutdown. |

Show cheap hints persistently (⌥Space, ⌘K, /, Esc); reveal the rest while Command is held. Native menu shortcuts use macOS localization/layout conventions. Do not register these local commands globally.

Sound cues (optional, independent of spoken content): capture-start = one soft 70 ms rising tone, submit = 80 ms falling pair, cancel = silent by default, error = one low 100 ms tone, response-complete = silent. Never play a start cue before capture actually starts; don't replay cues on rerender. Duck cues against speech; honor system volume and mute. These are synthesis specifications, not packaged recorded audio.

120 ms HUD appearance/disappearance; 180 ms expression/panel transitions. Never delay capture for an animation. No thinking spin timeout pretending progress. Reduce Motion removes breathing, pulsing and positional transforms, freezes menu animation and changes expressions directly. Reduce Transparency uses opaque native surfaces. The HTML uses opaque surfaces already.

## Accessibility and copy

SF Pro is deliberately chosen for the native Mac app, not an unconsidered web default. Body 13 pt, caption 11 pt, title 20 pt, display 28 pt; native text scaling follows platform preferences. Focus rings visible in both themes, no essential text below 11 pt. Token foreground/surface pairs are checked at ≥4.5:1, including muted and error copy. The large Face and state labels are redundant; screen readers do not have to infer mouth shapes.

VoiceOver names the status item “Softmax, [activity], [hub state]”; announcements on capture start/stop and errors are concise, not per-frame. Menu and Settings use native roles and predictable focus order. HUD is nonactivating unless explicitly opened for keyboard interaction; an accessible Talk/Stop button exists in the Face panel for users who cannot hold the shortcut. Permission recovery returns focus to the initiating control. Standard keyboard navigation and full keyboard access must work. A colorblind user can distinguish offline slash, error mark, listening bars and update dot.

Tone: brief, literal, calm. “Listening · release to send”, “Hub disconnected”, “Cancelled · nothing sent”, “Microphone access is off”, “Could not verify this hub”. Avoid “All systems optimal”, anthropomorphic claims about understanding, and technical node keys as primary labels. Place identity details behind Inspect. Do not promise all computation or audio processing stays on-device: processing placement is configured by capabilities and must be disclosed at enrollment.

## Distribution assets

`AppIcon.svg` is the editable 1024 master. `AppIcon.appiconset` has all ten required macOS slots (16/32/128/256/512 pt × 1×/2×), including 1024 px. `DMG/background.svg` is the 660×440 source; PNG exports at 660×440 and 1320×880. Installer positions: app at (180,230), Applications alias at (480,230), in a Finder window whose content area is 660×440; verify Finder coordinate origin in packaging. Arrow and instructional text are background only, never a fake application/folder icon. Onboarding vectors cover introduction, permissions and hub connection. `DesignTokens.json` is source; Swift values are generated, not hand maintained.

## v1 non-goals

No Dock icon by default; no web dashboard or developer trace explorer inside this app; no wake word/always-on capture; no transcript archive or audio retention by default; no auto-enrollment into an unverified hub; no silent capability expansion; no persona/voice marketplace; no multi-hub switching; no autonomous self-update during a conversation; no new expression protocol invented in UI code; no unsigned updater. This design package does not deliver a signed/notarized binary, acoustic qualification or native TCC permission proof.

## Verification and acceptance boundary

Run `bash scripts/check-assets.sh` and the Playwright recipe in the asset README. CI runs the asset checker. Screenshots in `Design/mockups/screenshots/` show both themes and all surfaces; state selectors make every modeled state reachable. Template proof images are mockup/browser renders on light/dark bars, **not native AppKit screenshots**. Native NSImage template inversion, SVG-to-PDF compilation, menu positioning, keyboard permissions, voice latency, microphone failure cancellation, Swift token compilation, Finder layout and VoiceOver require macOS integration verification before claiming an implemented app.

Slop self-audit: **0/10**, with SF Pro deliberately selected for native controls; no generic tech gradient, violet default, feature-card grid, accent rail, blur, monument metric, icon topper, center-stack product layout or wrong surface. The centered Face in the explicitly rejected Stage direction is intentional comparison, not the selected composition. App icon and About are appropriately centered identity surfaces. Review screenshots at normal size, not only via automated overflow assertions.
