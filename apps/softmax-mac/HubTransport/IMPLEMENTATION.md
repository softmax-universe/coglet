# ROUTER enrollment implementation notes

## Contract and boundaries

- Frozen `spec-v0:spec/hub-protocol.md`: pinned ROUTER identity, ALPN `coglet/hub/0`, peer-bound 256-bit invite, fresh enrollment per connection, null-invite recovery after ambiguous success. Short-code lookup and QR encoding are not wire methods. Keep existing code identifiers unchanged.
- All six prerequisite DONE receipts recovered from authorized replicas (PR12/48/58/74/101/114). Darwin subprocess validation explicitly refuses before spawn; Linux unreaped-leader custody is unchanged. Portable SDK CSPRNG repair is integrated.
- Configure-only extension: preserve Settings sidebar/search/title/DesignGroup, onboarding stages/illustration/login/voice/footer. Replace only the two enrollment placeholder rows with shared controls. Menu availability is orthogonal to voice/activity.

## Implemented, partial

`HubTransport/` is a standalone Cargo package. The production helper uses landed coglet-proto Session and real iroh. Key/config arrive only through anonymous pipes, never argv/env/files/logs. Output is sanitized state/public device ID. Commands are bounded to16KiB. Null-invite recovery precedes initial invitation; connected only follows version selection; QUIC closure emits offline; EOF closes the endpoint.

Swift HubAccount persists a32-byte generated-once seed before use, fails closed on Keychain errors and never silently rotates. HubKeychain injection tests use an in-memory backend; Security backend uses a generic password and `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`. Non-secret configuration uses UserDefaults; invitation stays in memory. HubConnection provides cancelable, generation-fenced reconnect with30s inclusive jitter cap and local sign-out deletion. ProcessHubLink bridges private pipes and fences stale callbacks.

## Verified native transport gate

Full logs from Softmax Mac run45386256577 at e0c953981e5a19dc367fb67c20b7180808ca0f0d were read independently:

- Four HubConnectionTests and two HubKeychainTests pass.
- Required `testHubConnectionTestsEnrollDropOfflineAndRecoverAgainstCogletMock` passes in1.796s, executing real helper and real coglet-mock launcher, not a fake or skip.
- Required `testProcessPublishesReadyBeforeItsOutputPipeCloses` passes in0.030s.
- This verifies the bounded4096-byte POSIX pipe-reader repair after both tests failed at initial deviceID on f441b29. EINTR retry,16KiB parser bound and generation fences are retained.
- The overall native job still FAILED later during app packaging, so it is not all-green acceptance.

Durable run: https://github.com/softmax-universe/coglet/actions/runs/45386256577/job/138417833696 . Worker raw logs: macos-45386256577-{full,failed}.log.

## Packaging repair and UI RED pending

The later native failure is `lipo: unknown architecture specification flag: <staging path>`: the input filename followed the variadic `-verify_arch` list. Tightened the portable lipo fake to enforce actual argument grammar; observed RED exit64. Production commit eb605f9 moves the input before `-verify_arch arm64 x86_64`; portable GREEN follows. All five portable packaging/signing tests also pass on5e07553 (portable-run1-5e07553.log). Fake orchestration is not native architecture/signature proof.

`scripts/build-hub-helper.sh` builds --locked --release for aarch64/x86_64 Apple targets, lipo-merges/verifies and atomically installs an executable under Contents/Helpers/softmax-hub-link. project.yml invokes it post-build. sign-dev.sh signs the helper before the outer app; release recursively signs Mach-O. Native packaged-helper resource/signature proof remains required.

PR98 is OPEN/DRAFT at5e07553c39445e5c7f353ba2a812e18c98a5e1c2. Native run45391611490 at that exact head is IN_PROGRESS. It contains the packaging repair and a new test-only UI RED probe, `testRouterEnrollmentConfigurationInOnboardingAndSettings`. The test expects shared `Pair router…` entry points, a pairing sheet with `Pairing code`, `Import QR image…`, a disabled empty `Connect router`, Cancel return, and preserved Settings navigation/onboarding login/footer. Production UI is still unchanged. Read actual failure logs before implementation; packaging/compiler failures do not establish intended UI RED. Earlier packaging-only run45391423072 was superseded/cancelled by this push.

## Next implementation slice

UI baseline gate PASS in Design/hub-enrollment-baseline.md. Prior normal light/dark Settings and final onboarding renders were inspected, capture times unknown, publication provenance recorded, marked stale against newer source. Native post-change large-text/high-contrast comparison remains required; baseline status is unverified.

Use a shared compact connection summary plus `Pair router…` sheet in the existing Settings Hub/onboarding group, not a new wizard/navigation. The sheet progressively discloses developer pinned manual address entry; it imports a local QR image or accepts pasted provisioning JSON. Envelope fields: name/hub/addresses plus optional relay/invite/device. Validate through HubPairing; do not invent a remote short-code lookup. Display/copy public device ID before invite provisioning. Preserve token fonts, cancel/default actions, and optional onboarding completion without enrollment. Clear pasted invitations on cancellation/success; never persist or log them.

Still required:

1. Verify native lipo repair and intended missing-UI RED in45391611490.
2. Implement shared controls, QR import, manual pinned address, public ID/name/status/sign-out; wire ShellModel startup/stop and existing menu availability independently of voice. UI-testing/snapshot mode must not access real Keychain or network.
3. Exercise native UI behavior and render comparison (normal/large/contrast), updating only deliberately reviewed changed snapshots.
4. Verify actual universal helper in the app bundle, resource lookup, inside-out signing; revisit private-pipe SIGPIPE and process-crash/restart regression coverage.
5. Independent exact-head review, every required check green, repository merge queue, then kanban_request_review with published_pr after merge. No delivery claim yet.

## TDD provenance

Rust initial library RED then real-iroh GREEN. Swift missing core API RED45337370014; missing process bridge RED45339321655; missing Keychain injection RED45339491432; focused native readiness RED45382448795 then GREEN45386256577. UI behavior probe is pending, not a claimed RED. Linux worker has no Swift toolchain; native results are hosted evidence, never local execution. Earlier Rust real-iroh two-test/strict-Clippy/fmt passes at4d8c674 are historical, not rerun during this packaging/UI-probe slice.
