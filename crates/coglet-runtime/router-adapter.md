# Router-managed device lifecycle

`cogletd --router-service CONFIG.json` serves `hub.git.push`, `hub.install`,
`hub.start`, `hub.stop` and `hub.status` over authenticated `coglet/hub/0`.
This is the device path, not the CLI→router deploy path or full protocol qualification.

## Admission and durability

- Push uses shared streaming Git, quarantine validation and `AcceptedRefStore`.
  An authorized uninstalled target may retain a validated bundle and queued
  admission receipt; it cannot execute. Admission is distinct from installation.
- Install revalidates the retained signed router commit and its immutable contents.
  Root `manifest.json`, `bin/step.wasm` and `build/provenance.cbor` must be in the
  signed tree. Manifest capability admission intersects the local catalog/grants.
  No compiler runs on the device. `/code` is capped at 8 MiB raw bytes; private
  `/data` starts empty. A per-target WAL pins install identity and original receipt.
  Identical install returns that receipt; a different accepted install conflicts.
- Authority is operator-provisioned, never populated by a remote install proof.
  `runtime_boundary` holds SQLite `BEGIN IMMEDIATE` across fresh exact peer,
  actor/ticket, method, generation, delegation and trusted-expiry checks and the
  synchronous runtime WAL application. Install requires the exact current proof;
  install/start also compare the accepted ref inside this guard. Other authority
  connections cannot revoke or replace pins halfway through application. The
  trusted clock and request deadline are rechecked after coordinator-lock waits
  and native-engine preparation, immediately before application or disclosure.
- Start revalidates the signed bundle, manifest grants and explicit kill-switch
  JSON `{ "latched": false }`; absent, unreadable or malformed kill state refuses
  start. It enables the qualified native engine and persists running. Builds
  without `native-engine` refuse start; there is no mock fallback.
- Stop obtains exclusive coordinator access after the current bounded step.
  It durably prevents further execution, cancels unexposed actions through existing
  terminal-rejection staging, and permanently fences retransmission of prepared
  envelopes. Historical adapter accept/outcome receipts remain reconcilable;
  already exposed physical effects cannot be retracted or given invented outcomes.
  Unknown engine attempts retain their exact durable recovery fence. Cancellation
  queue pressure leaves the coglet stopped and reports failure rather than success.
  Stop does not depend on a newer pushed ref remaining valid/available.
- Status projects the frozen `hub-status` fields. Stored installed/stopped state
  and original receipt survive process death and WAL reopening.
- Start/stop persist peer/method/target/params/actor-ticket intent digests and
  original receipts in the same WAL frame as the transition. After fresh
  authorization, an identical retry cannot undo a later transition, including
  across restart. Changed intent on the same operation ID conflicts. The bounded
  ledger retains 4096 operations and never evicts retry evidence. Once full it
  refuses new starts (`resource_exhausted`), while a new stop still commits the
  durable fence and returns an applied receipt that is not retained; no later
  start can exist for a replay of that stop to reverse. Receipt polling remains
  a separate, unimplemented method.

`hub.git.fetch`, debug, actions, inbox, receipt polling and cancellation methods
are not implemented by this handler and fail closed. This is not a replacement
for their independent acceptance gates.

## Configuration and runtime API

The default-enabled `device` Cargo feature exposes `DeviceHandler` and builds
`cogletd` plus its validator. Normal daemon builds retain that feature. Embedded
C ABI consumers use `default-features = false` and enable `native-engine` only;
the Apple xcframework/link-probe scripts select this profile explicitly. This
keeps daemon-only iroh/proto dependencies (including desktop DNS APIs) out of
iOS archives. It does not qualify the router transport on iOS. Check the actual
normal dependency graphs for all four Apple targets with
`python3 crates/coglet-runtime/tools/check-embedded-dependencies.py`; archive
linking and native execution still require the hosted Apple jobs.

`DeviceHandler::new(config, authority, validator_path, clock)` takes local pins,
a pre-provisioned `Authority`, a trusted helper executable, and
`Arc<dyn Fn() -> Option<u64> + Send + Sync>` (trusted Unix seconds, `None` refuses).
`DeviceConfig` contains `hub`, `device`, private `wal_root` and `quarantine`,
`catalog`, `grants`, `kill_switch`, `targets` and optional `adapter`.
Each `TargetPin` has separate `grant_generation` and `delegation_generation`,
and an optional `native_schedule` (`trace_id`, `seed`, `runtime_bytes`, `stack_bytes`).

The daemon's closed JSON configuration additionally supplies `identity_path`,
`authority_path`, `validator_path`, optional `invite` and `hub_addr` (IP:port).
It checks the private identity file against the pinned device, dials the pinned
router, enrolls as `Role::Device`, and serves the handler. It does not reprovision
or silently migrate authority on startup. Identity/invite bytes are never logged.
Install `coglet-runtime-git-validator` beside `cogletd`; it is the shared bounded
validator entry point, not another validator implementation or a compiler.

`DeviceHandler::share_service` returns a trusted in-process coordinator handle,
not a remote capability. Optional `AdapterConfig` contains socket, binding file,
peer uid and one coglet. Its Unix listener shares this coordinator. Native steps
require a connected, granted adapter `SurfaceSession` pump; a running receipt
alone does not prove a guest step. Listener binding currently reports errors to
stderr; no adapter session means no guest steps. The native execution test below
exercises the installed coordinator directly, separately from the daemon wire test.

## Requirement-to-test matrix

Tests run against **coglet-mock `MockHub`**, using real iroh sessions and real signed
Git bundles, not the concurrently developed reference-router deployment operator.
The Git validator is Linux-qualified; device wire tests are explicitly Linux-only.

| Requirement | Named test (`tests/device_lifecycle.rs`) |
| --- | --- |
| push cannot install/run; repeated receipt | `push_install_stopped_then_stop_and_repeat_receipt` |
| install/start/running/stop | `install_start_running_stop_with_native_engine` |
| expired, wrong node/signer/generation | `install_rejects_wrong_expired_and_wrong_generation_signed_delegations` |
| exact methods/generation pins | `wrong_exact_method_and_wrong_generation_pins_are_denied` |
| actor/ticket/peer/target | `unauthorized_peer_context_ticket_and_target_are_denied` |
| unverified code refused | `install_without_accepted_push_is_refused`, `start_refuses_tampered_retained_bundle_bytes`, `start_refuses_after_authority_loses_the_accepted_ref` |
| differing install conflicts; stop survives pushed advance | `start_refuses_when_accepted_ref_advances_past_installed_commit` |
| restart state/receipt | `wal_restart_preserves_installed_and_stopped_state` |

Additional process/native/kill-switch tests in that file cover actual `cogletd`
SIGKILL/restart, a real NativeEngine guest step with committed journal readback,
and latched/malformed/missing kill-state refusal. Managed coordinator tests in
`tests/managed_lifecycle.rs` separately prove safe stop, cancellation, unknown
attempt fencing, WAL reopen, native scheduling fences and legacy behavior.
Run `cargo test --locked -p coglet-runtime --test device_lifecycle` and repeat
with `--features native-engine`; the default route intentionally proves no-native
start refusal. Full workspace tests, serial native tests, both Clippy routes with
`-D warnings`, fmt and diff checks remain mandatory before landing.
