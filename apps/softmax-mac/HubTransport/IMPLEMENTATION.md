# Hub enrollment implementation notes

## Pinned inputs

- Base `736db6ead9263e31b00680d3632cf269e19a5b38` includes mock hub PR74.
- Original shell/kernel/protocol parents and new mock part-1 parent are DONE in authorized replicas.
- `spec-v0:spec/hub-protocol.md` requires pinned hub identity, ALPN `coglet/hub/0`, peer-bound 256-bit invite, fresh enrollment on every connection, and null-invite recovery after ambiguous success. No short-code lookup or QR encoding is a wire method.

## Selected composition and boundary

Configure surface: preserve Settings sidebar, heading and DesignGroup, replace only its Hub placeholder. Preserve onboarding steps, existing permissions/login controls and footer; replace Hub placeholder with the same reusable enrollment component. Existing design tokens and keyboard controls remain. Menu status remains orthogonal to activity; wire current hub connection into the existing availability input. Native render comparison is still required; no UI implementation or visual acceptance is claimed yet.

App transport is `HubTransport/`, an isolated Cargo package to avoid root workspace edits. The helper uses the landed coglet-proto Session API and real iroh, never a substitute socket protocol. Identity/config arrive through anonymous pipes, not argv/environment, files or logs. Stdout carries sanitized state and public device ID only; command parser bounds input to 16 KiB. The helper attempts null-invite recovery before consuming an initial invite, pins the hub node, reports connected only after version selection, and reports offline after QUIC closes. EOF closes its endpoint.

## Remaining Swift implementation (not delivered)

1. `HubStore` main-actor abstraction: key and non-secret hub configuration, with an in-memory fake. `HubAccount.identity()` loads existing 32-byte seed or generates once, saving before use; any read/write failure fails closed without rotating. Real adapter uses Security generic-password items and `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`; non-synchronizable. Non-secret configuration may use UserDefaults, never an invite.
2. `HubLink` main-actor adapter for the bundled helper; private input pipe and async output reader. Stop terminates helper, closes handles, cancels reader. Generation fence ignores late process callbacks after sign out/new start.
3. `HubConnection` owns observable state via a callback (portable core avoids Combine); automatic exponential capped backoff, bounded jitter, cancelable retry; immediate offline on disconnect, reconnect via same identity and null invite. Sign out stops tasks/helper before deleting both local key and hub record. Show deletion/storage errors rather than silently succeeding.
4. Pairing: accept pasted QR JSON or image QR import locally; developer fields for pinned hub ID and socket addresses, 64-lowercase-hex invitation. Display/copy public device ID first for peer-bound invitation creation. Local JSON envelope: name, hub, addresses, optional relay/invite/device. Validate limits, hex, addresses and expected device; never introduce a network short-code service or change spec.
5. Add real native `HubConnectionTests` using production helper plus `coglet-mock --example hub DEVICE_NODE_ID` launcher. Assert connected, force drop, observe offline, then auto-recover same identity with null invite. Existing initial tests cover fake keychain/restart/failure, controller reconnect and late callbacks.
6. Build universal helper, copy it into app bundle on project build, sign inside-out in dev path (release script already recursively signs Mach-O). Minimal workflow steps build helper/mock launcher and set test executable paths. Require native test execution, not skip-only success.

## Continuation 3 (partial, not acceptance)

- Native run45339491432@95819e4 confirmed intended missing HubKeychain/HubKeyAccessibility and extra `keychain:` argument RED. Unrelated snapshot "missing actual" ran after the compile failure (`if: !cancelled`) and is not RED.
- Implemented HubKeychain protocol, HubKeyAccessibility.afterFirstUnlockThisDeviceOnly, SecurityHubKeychain (generic password, device-only accessibility, add-only), and KeychainHubStore(keychain:).
- Capped reconnect delay including jitter at 30s. Early workflow filter now includes HubKeychainTests.
- ProcessHubLink remains from 3e7466b. No UI/signing/universal packaging yet. No native GREEN claimed until hosted Swift runs.

## Continuation 2 (partial, not acceptance)

- Actual macOS run45337370014 confirmed intended missing HubStore/HubPairing/HubLink RED before Swift implementation.
- Implemented HubAccount/KeychainHubStore, validated local HubPairing, and generation-fenced HubConnection at43cfba1. Native run45339043426 found a Swift 6 actor-isolation error in the default random generator; fixed with `nonisolated static` atd20572b.
- Native run45339321655 then compiled the new production core and failed as intended for missing ProcessHubLink in the new real-mock process acceptance test. Actual logs read. Implemented that process bridge at3e7466b (backup only, not yet natively compiled).
- Added HubKeychainTests at95819e4: requires injected HubKeychain backend and HubKeyAccessibility policy. Those APIs are intentionally NOT implemented yet. Run45339491432 on exact95819e4 is queued; obtain its missing-keychain-API RED before refactoring KeychainHubStore. PR98 deliberately remains95819e4 so this native RED is not canceled. Backup branch wip/t_01a205f93f231486 holds newer3e7466b, including process bridge and workflow real-helper/mock builds plus required native acceptance execution.
- No Swift GREEN yet. No UI, signing or universal packaging yet. Rust tests are prior-run results on unchanged Rust source; no fresh Rust execution claimed this run.
- Next: verify Keychain RED; implement HubKeychain abstraction/default Security backend and injected in-memory tests. Push latest source to PR branch, get core/native compile+GREEN and actual real mock-hub acceptance. The native bootstrap reader timeout is canceled as soon as bootstrap arrives; no secret output is logged.
- Revisit process crash recovery/error tests, bounded retry cap including jitter, and pipe-write SIGPIPE behavior. Identity, enrollment and sign-out remain fail-closed. Preserve the UI delta boundary above; full native render comparison, independent review, all CI and merge queue remain required.

## TDD state

Rust `enroll_drop_offline_recover_same_identity` was observed RED (unresolved app library) before implementation, then GREEN against real MockHub. Private stdio process integration, EOF and strict Clippy also passed locally. Swift tests were pushed before any production Swift implementation: draft PR98 at initial head3dd8ac0; macOS run45337370014 is queued. Its intended RED must be verified from actual compiler/test logs before implementing the Swift account/controller. This Linux host has no Swift toolchain; don't claim native execution.

## Gates still open

No app identity/UI/controller exists yet. No end-to-end Swift acceptance, native rendered comparison, independent final review, green final CI or merge claim. PR98 intentionally remains DRAFT. Do not request acceptance review or complete the card at this checkpoint.
