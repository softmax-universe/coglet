# ADR-0037: Baby Bear iOS command disposition for the embedded cutover

- Status: Proposed (design only; no phone cutover authorized by this document)
- Date: 2026-11-25
- Authors: Coglet contributors
- Related task: `t_01a211fcd5927efb`
- Related PR: recorded in the accepting review
- Supersedes / superseded by: none; narrows the product cutover scope under ADR-0022

## Context

[ADR-0022](0022-embedded-product-catalogs.md) approves only the closed
`system.status` command observation and `babybear.system_status` intent.
The [iOS integration](../integrations/babybear-ios.md) is an opt-in slice,
not a migration of the entire PodHost. Before qualification, every command
reachable from `NativeCapabilities.install` needs an explicit disposition.

This inventory pins Baby Bear main `749245d4a92d7b2f3624732ef311af146afc1ee3`.
It is the union of registrations on a successful install across hardware/API
branches, NOT a claim that one simulator or phone exposes every command.
Foreground, protected-data and permission checks still apply at execution.

## Decision

Retain the existing guest admission for `system.status`; move **no additional
commands** behind guest admission in this cutover. The other 50 commands stay
native, including their admission, cancellation, permission and receipt paths.
This is a deliberately bounded cutover, not a statement that native effects
cannot later be gated by guest intents. Native OS ownership alone would not
preclude such a gate: it is the absence of a reviewed closed mapping and observed
parity for these stateful/private effects that makes expansion inappropriate now.

The sole guest-backed route still invokes native telemetry once, through the
installed handler after the matching durable intent. It does not move UIKit,
AVFoundation or filesystem sampling into Wasm. Native remains the product default
until the software and physical-phone gates below are satisfied. Selecting coglet
later means selecting this finite status admission slice, not all pod behavior.

No wire, ABI, catalog, schema, resource, permission, entitlement, immutable release
or pin changes. Keep ADR-0022's empty status arguments and one command owner.
No second microphone, speaker, camera, stream or command executor in shadow.
Never replay claimed/running/unknown IDs, nor fall back to native after guest
failure. Rollback remains one tap, future-only, preserving every durable ledger.

### Complete install-tree inventory

`NativeCapabilities.swift` calls `installRest`, which calls
`NativeDataCapabilities.install` and `installSecondWave`. The latter installs
camera, microphone, haptics, location, motion, reminders, health, clipboard,
web-panel and Bluetooth adapters. `NativePermissions` adds no command.
The source directory is `pods/ios/Sources/BBIOSCapabilities/` in Baby Bear;
links below pin the exact registration line. The media/playback loop contributes
six distinct commands; the protected-data tuple loop contributes four.

Each row is a disposition, not a hardware test result. **S** = existing guest
admission only (native telemetry); **N** = stays entirely on the existing native
command path. Reasons bound this cutover and are not permanent portability claims.

| Command | Disposition | Registration at pinned source | Reason |
| --- | --- | --- | --- |
| `ios.bluetooth.state` | N | [BluetoothAdapter.swift:224](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L224) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.scan` | N | [BluetoothAdapter.swift:234](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L234) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.connect` | N | [BluetoothAdapter.swift:266](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L266) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.disconnect` | N | [BluetoothAdapter.swift:293](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L293) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.services` | N | [BluetoothAdapter.swift:303](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L303) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.read` | N | [BluetoothAdapter.swift:318](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L318) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.write` | N | [BluetoothAdapter.swift:331](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L331) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.notify` | N | [BluetoothAdapter.swift:342](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L342) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.connected` | N | [BluetoothAdapter.swift:377](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L377) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `ios.bluetooth.advertise` | N | [BluetoothAdapter.swift:386](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/BluetoothAdapter.swift#L386) | CoreBluetooth radio authorization, BLE connection/GATT/subscription/advertisement lifetimes and cancellation require native ownership. |
| `system.status` | S | [NativeCapabilities.swift:139](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L139) | Native UIKit/ProcessInfo/filesystem/AVAudioSession telemetry remains host-owned; guest may own only existing status command admission/intent, not OS sampling. |
| `system.brightness` | N | [NativeCapabilities.swift:145](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L145) | UIScreen brightness mutation and immediate readback require the iOS process/UI boundary. |
| `media.play` | N | [NativeCapabilities.swift:146](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L146) | AVPlayer decoding/output and shared AVAudioSession reservation, completion and cancellation are native resources. |
| `media.status` | N | [NativeCapabilities.swift:147](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L147) | Reads live AVPlayer/speech instance state and elapsed time; not a guest-owned playback model. |
| `media.pause` | N | [NativeCapabilities.swift:149](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L149) | Controls retained AVPlayer state; playback.* additionally controls AVSpeechSynthesizer. Native audio ownership/cancellation cannot be modeled by guest aliases alone. |
| `media.resume` | N | [NativeCapabilities.swift:149](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L149) | Controls retained AVPlayer state; playback.* additionally controls AVSpeechSynthesizer. Native audio ownership/cancellation cannot be modeled by guest aliases alone. |
| `media.stop` | N | [NativeCapabilities.swift:149](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L149) | Controls retained AVPlayer state; playback.* additionally controls AVSpeechSynthesizer. Native audio ownership/cancellation cannot be modeled by guest aliases alone. |
| `playback.pause` | N | [NativeCapabilities.swift:149](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L149) | Controls retained AVPlayer state; playback.* additionally controls AVSpeechSynthesizer. Native audio ownership/cancellation cannot be modeled by guest aliases alone. |
| `playback.resume` | N | [NativeCapabilities.swift:149](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L149) | Controls retained AVPlayer state; playback.* additionally controls AVSpeechSynthesizer. Native audio ownership/cancellation cannot be modeled by guest aliases alone. |
| `playback.stop` | N | [NativeCapabilities.swift:149](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L149) | Controls retained AVPlayer state; playback.* additionally controls AVSpeechSynthesizer. Native audio ownership/cancellation cannot be modeled by guest aliases alone. |
| `speech.say` | N | [NativeCapabilities.swift:157](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L157) | AVSpeechSynthesizer callbacks and shared audio-session ownership must remain bound to the native utterance. |
| `artifacts.show` | N | [NativeCapabilities.swift:164](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L164) | Native artifact authorization, UIKit/Lens presentation, AVPlayer video/PiP and WebKit document lifetime remain host-owned. |
| `artifacts.windows` | N | [NativeCapabilities.swift:185](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L185) | Reads actual UIKit visibility and the native current artifact/window state. |
| `artifacts.window.close` | N | [NativeCapabilities.swift:186](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L186) | Dismisses native presentation and cancels native media/document state. |
| `apps.open` | N | [NativeCapabilities.swift:255](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L255) | UIApplication/app opener handoff backgrounds BB; native opener and original-command background receipt own the effect. |
| `apps.status` | N | [NativeCapabilities.swift:264](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L264) | Queries declared schemes through UIApplication.canOpenURL and native enablement/inventory state. |
| `notifications.post` | N | [NativeCapabilities.swift:265](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeCapabilities.swift#L265) | UNUserNotificationCenter owns scheduling/cancellation; Focus/lock delivery is outside guest observation. |
| `ios.calendar.events` | N | [NativeDataCapabilities.swift:22](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeDataCapabilities.swift#L22) | EventKit authorization and bounded native calendar-window query cannot be replaced by guest data ownership. |
| `ios.contacts.search` | N | [NativeDataCapabilities.swift:22](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeDataCapabilities.swift#L22) | Contacts authorization and bounded name-only CNContactStore enumeration stay in the protected native process. |
| `ios.files.read` | N | [NativeDataCapabilities.swift:22](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeDataCapabilities.swift#L22) | App-granted user-selected file access and chunk bounds belong to native GrantedFiles, not arbitrary guest paths. |
| `ios.photos.list` | N | [NativeDataCapabilities.swift:22](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/NativeDataCapabilities.swift#L22) | Photos authorization/limited-library metadata handles are app-local, permission-scoped native APIs. |
| `cameras.list` | N | [WaveAdapters.swift:67](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L67) | AVFoundation camera discovery/capture/torch hardware, capture cancellation and private capture-store artifacts require native ownership. |
| `cameras.capture` | N | [WaveAdapters.swift:72](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L72) | AVFoundation camera discovery/capture/torch hardware, capture cancellation and private capture-store artifacts require native ownership. |
| `ios.torch.set` | N | [WaveAdapters.swift:99](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L99) | AVFoundation camera discovery/capture/torch hardware, capture cancellation and private capture-store artifacts require native ownership. |
| `ios.microphone.record` | N | [WaveAdapters.swift:132](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L132) | AVAudioRecorder/input route, audio-session reservation and stop/abort/file cleanup must remain native. |
| `ios.microphone.stop` | N | [WaveAdapters.swift:141](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L141) | AVAudioRecorder/input route, audio-session reservation and stop/abort/file cleanup must remain native. |
| `ios.haptics.play` | N | [WaveAdapters.swift:173](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L173) | UIKit haptic feedback generators trigger physical hardware without proving vibration; host owns the effect. |
| `ios.location.current` | N | [WaveAdapters.swift:193](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L193) | CoreLocation authorization, sensor streams, accuracy/timeout and stop callbacks remain native and foreground-bound. |
| `ios.location.heading` | N | [WaveAdapters.swift:217](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L217) | CoreLocation authorization, sensor streams, accuracy/timeout and stop callbacks remain native and foreground-bound. |
| `ios.motion.snapshot` | N | [WaveAdapters.swift:249](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L249) | CoreMotion history and barometer callbacks/cancellation depend on native sensor availability and permission. |
| `ios.reminders.list` | N | [WaveAdapters.swift:289](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L289) | EventKit authorized reminders fetch and cancellation remain in the app permission boundary. |
| `ios.health.read` | N | [WaveAdapters.swift:335](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L335) | HealthKit protected read queries, cancellation and non-disclosure of read denial must remain host-owned. |
| `ios.health.summary` | N | [WaveAdapters.swift:349](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L349) | HealthKit protected read queries, cancellation and non-disclosure of read denial must remain host-owned. |
| `ios.health.types` | N | [WaveAdapters.swift:353](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L353) | HealthKit protected read queries, cancellation and non-disclosure of read denial must remain host-owned. |
| `clipboard.write` | N | [WaveAdapters.swift:393](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L393) | UIPasteboard and explicit on-phone consent UI require native ownership; guest must not bypass that privacy boundary. |
| `clipboard.read` | N | [WaveAdapters.swift:397](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L397) | UIPasteboard and explicit on-phone consent UI require native ownership; guest must not bypass that privacy boundary. |
| `browser.open` | N | [WaveAdapters.swift:446](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L446) | WKWebView navigation/HTML/snapshot and presentation lifetime stay native; this is only BB-owned web content. |
| `browser.list` | N | [WaveAdapters.swift:449](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L449) | WKWebView navigation/HTML/snapshot and presentation lifetime stay native; this is only BB-owned web content. |
| `browser.close` | N | [WaveAdapters.swift:454](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L454) | WKWebView navigation/HTML/snapshot and presentation lifetime stay native; this is only BB-owned web content. |
| `ios.browser.html` | N | [WaveAdapters.swift:458](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L458) | WKWebView navigation/HTML/snapshot and presentation lifetime stay native; this is only BB-owned web content. |
| `ios.browser.screenshot` | N | [WaveAdapters.swift:461](https://github.com/softmax-universe/babybear/blob/749245d4a92d7b2f3624732ef311af146afc1ee3/pods/ios/Sources/BBIOSCapabilities/WaveAdapters.swift#L461) | WKWebView navigation/HTML/snapshot and presentation lifetime stay native; this is only BB-owned web content. |

Registration may be hardware-conditional: camera list/capture need a camera;
torch additionally needs a torch; microphone, motion, haptics, HealthKit, heading
and Bluetooth depend on backend availability. An absent command is not silently
emulated by the guest. `playback.*` also controls speech, unlike `media.*`.
UI and data wrappers remain native even where they are technically portable:
their private observations and native lifetime bindings are outside ADR-0022.

## Alternatives

- Gate all commands via a generic command envelope: rejected; it widens the
  closed catalog without per-command privacy, identity and resource contracts.
- Start a second native handler in shadow: rejected; parity cannot actuate twice.
- Move read-only private queries first: deferred; read-only is not permission to
  put Contacts, HealthKit, Photos or live application state into recorded events.
- Treat a selected-default status slice as full Pod migration: rejected; all 50
  N rows remain native, and must be reported as such at handoff.

## Consequences

This decision closes inventory/design scope only. It does not waive ADR-0022's
required tests, certify the simulator, authorize a phone install, or make selected
the default. Item 2 software qualification remains ahead of physical cutover.
No new guest contracts are required for N rows because none moves. A later move
requires a reviewed closed command contract, privacy/resource ownership, recorded
shadow observation without another actuator, selected-mode ownership and rollback,
and matching simulator side-by-side evidence before inclusion.

## Verification

- Source inventory: enumerate literal registrations plus both complete loops;
  require 51 unique commands, 41 literal callsites and 10 expanded loop entries,
  with exactly one S row. Check transitive installer callsites, not just strings.
- `PodCogletContractTests`, `PodCogletEmbeddedTests`, `PodCogletParityTests`,
  `PodCogletShadowTests`, `PodCogletRollbackTests`, `PodCogletSelectionTests` and
  `PodCogletLaunchTests` retain ADR-0022's required simulator obligations.
  Positive named execution, zero failures/skips and the final
  `ios-targeted-ledger` tested-source revision are mandatory; source inspection,
  Linux Rust proof and build-for-testing are not simulator execution.
- Still REQUIRED, not claimed passing here: same durable unknown IDs across actual
  app relaunch; no fresh hub dispatch while locked/backgrounded; exact unknown
  receipt upload; actual guest trap and budget exhaustion through the iOS runtime.
- Physical iPhone lock/background/suspend/relaunch/protected-data qualification and
  recorded one-tap future-only rollback follow software gates. Request David's
  exact phone steps only then. Native default stays unchanged until that gate.

## Approval

Proposed for independent exact-head review and normal required CI/merge queue.
No implementation or device qualification is implied by accepting this inventory.
