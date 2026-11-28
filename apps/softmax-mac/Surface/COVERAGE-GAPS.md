# Native catalog coverage qualification gaps

Source investigation at `f1ffd497628f455291103a9788180f9c139214e7`, not native
execution or an acceptance waiver. Keep the explicit coverage ledger unresolved
until actual AX evidence passes. The existing welcome omission negative control
already removes a real native mount (`SoftmaxUITests.testCatalogCoverageDetectsNativeOmittedRegistration`);
do not recreate it or confuse it with that file's separate captured-evidence mutation.

## Next executable checkpoint

`SoftmaxCatalogCoverageExtensionTests.testCatalogCoverageMenuBarNativeStatusItem`
is a new, unqualified test: discover the actual app-owned status item by its public
native title, click it, enumerate the full popup descendant multiset, exercise only
ordinary Open Chat, then check dismissal/reopening and registration retirement.
No production repair or synthetic menu fixture accompanies this test. On failure,
read its `menu-bar-status-item-discovery` / `menu-bar-popup-discovery` hierarchy
attachments before changing selectors. Never count this test's source as native
coverage, and never remove the menu ledger gap solely because it exists.

## Remaining presentation drivers

- **main**: no native main window currently exists. The real-wire
  `test_softmax_app_observe_views_and_focus` now asserts exactly one closed/null
  record, an empty successful controls read and no native `softmax.main` window
  before/after each attempted operation. Open/navigate fail with wire `unavailable`;
  closing an already-closed window is a successful no-op before host dispatch,
  not an unsupported error. The new assertions remain natively unqualified.
  Do not fabricate a screen or label this an actionable tree census.
- **orb**: `OrbButton.installVoiceAccessibilityAction` already exposes an actual
  `NSAccessibilityCustomAction("Open Voice")`. Right-click and that action enter
  `performVoice`; `OrbVoiceRegistration` is a `gesture`/`press` registration.
  A node-only XCUI census cannot independently inventory the secondary action.
  Inspect actual native accessibility objects/actions (without consulting the
  registry) and bind the action to its real owning button. Do not add a fake
  second button merely to make a node count agree. Native custom-action and
  semantic-parent evidence remain unqualified.
- **hud**: `TalkHUD.update(model:)` consumes `ShellModel.talkState`,
  elapsed and level directly; a DEBUG + isolated UI-testing presentation fixture
  can call the existing `onTalkChange` owner callback without starting capture,
  global hotkeys, runtime or permission requests. The real borderless panel has
  static text and a listening progress indicator, **zero actionable controls**.
  A passive checkpoint needs positive native anchors, explicit zero-action
  enumeration and snapshot agreement, not the interactive helper's nonempty
  requirements. Exercise finite visible states and cancellation/hide lifecycle.
- **sign_out_confirmation**: the new DEBUG-only `CatalogSignOutFixture` implements
  the prerequisite without enrollment: real `HubConnection.start()` identity-load
  failure from an in-memory throwing store, inert link, then synchronous `stop()`
  before MainActor yields. The validated private catalog launch additionally
  requires `--catalog-sign-out-startup-error`; runtime must be stopped and no
  connection may exist. No credentials, Keychain, network or storage mutation.
  `testCatalogCoverageSignOutConfirmationCancelAndReopen` uses actual Settings/Hub
  navigation and Sign out, strict two-button AX/registry census, **Cancel only**,
  full mount retirement, reopen/cancel and unchanged closed fixture counters.
  `testCatalogSignOutFixtureIsNotSelectedByDefault` retains the ordinary launch
  control. Four fixture markers extend the Release exclusion/Debug-positive gate.
  These new native tests and actual Mach-O marker checks remain unqualified;
  the ledger is intentionally unchanged until native execution succeeds.
- **quit_confirmation**: `AppDelegate.quit()` checks `model.state` (not simply
  talkState) before showing its `NSAlert.runModal`. A safe fixture must establish
  the real state prerequisite, not trigger microphone or actual conversation.
  Enumerate the exact alert and its two native buttons, then **Cancel only**.
  Keep `hasUnqualifiedAppModal` snapshot/mutation refusal intact. ADR-0030 permits
  specifically identified OS-owned dialogs outside the semantic tree; do not
  fabricate an app-owned registered window or broadly exclude modal content.

## Other open gates

Per-node role/verb and hierarchy coverage is not established merely by the current
ID-set equality. Full dynamic-state coverage remains required. Product conformance
currently runs only Python-reference actions; it must still drive the same cases
through the native app and its mock, including reads and committed events.
The shared profile/harness owner PR #171 is now merged; inspect fresh main and
coordinate minimal shared workflow edits before integrating those seams. No
shared workflow or harness edit was made by this investigation.
