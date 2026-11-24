# ROUTER enrollment implementation notes

## Boundaries

Frozen `spec-v0:spec/hub-protocol.md`: pinned ROUTER identity, ALPN `coglet/hub/0`, peer-bound 256-bit invite, null-invite recovery after ambiguous success. Local pairing-envelope and QR-image decoding are provisioning UI, not a new wire method or remote short-code lookup. Code identifiers remain unchanged. All prerequisite DONE receipts recovered through authorized replicas (PR12/48/58/74/101/114). Darwin validator refuses before spawn; Linux custody is unchanged.

Configure-only UI extension preserves Settings sidebar/search/title/DesignGroup and onboarding stages/illustration/login/voice/footer. Menu availability is independent of voice activity. Baseline checker passed before edits; post-change native visual comparison is still required.

## Implemented, not yet accepted

- App-owned Rust helper uses real iroh and landed coglet-proto Session. Key/invitation cross anonymous pipes only, never argv/env/files/logs. Bounded16KiB commands; output contains sanitized states/public device ID. Null-invite recovery precedes initial invitation. QUIC loss emits offline; EOF closes endpoint.
- Swift HubAccount persists generated32-byte seed before use, fails closed on storage errors, and never silently replaces an enrolled missing key. Injected HubKeychain supports memory tests; Security backend uses add-only generic password with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, no iCloud synchronization. UserDefaults contains non-secret configuration; invite remains memory-only.
- HubConnection provides generation-fenced retries with30s inclusive jitter cap, persisted enrollment without invite, offline status and local sign-out. ProcessHubLink uses bounded POSIX read with EINTR retry: Foundation fill-to-count read stalled the persistent pipe's short readiness frame.
- Shared HubSettings summary and pairing sheet are wired into Settings and final onboarding. Pasted local envelope, local CoreImage QR import, developer manual pinned node/address/relay/invite, public identity copy/name/status/signout. Testing/snapshot mode returns before Keychain/transport construction. Inputs cleared on cancel/submission/dismissal. ShellModel startup/shutdown and menu status are wired.
- Universal helper build installs Contents/Helpers/softmax-hub-link. Xcode post-build uses --locked release Apple targets and correct lipo grammar (input before variadic -verify_arch). Dev signing signs helper before outer app.

## Established native evidence

- Account missing-API RED45337370014; bridge RED45339321655; injected Keychain API RED45339491432.
- Focused persistent-pipe readiness RED45382448795, then GREEN45386256577@e0c9539. Required real `testHubConnectionTestsEnrollDropOfflineAndRecoverAgainstCogletMock` PASS1.796s, focused readiness PASS0.030s, six core tests pass; no skips in required native lane. Overall job later failed packaging, not transport.
- Strict portable lipo fake exposed invalid argv REDexit64; production eb605f9 fixed argument order. Native45391611490@5e07553 produced real universal arm64+x86_64 helper, independently confirmed with file(1) after downloading app artifact. Not signature proof.
- Native45391611490 UI RED at SoftmaxUITests.swift:315: final onboarding lacked Pair router entry.
- UI implementation b4f0e9e then native45398673357 compiler RED: catch binding shadowed State error. Fixed with self.error in fa2c304.
- At this checkpoint native45403353278@fa2c304 is IN_PROGRESS in app/unit/UI step; account, required real coglet-mock, runtime and portable lanes already succeeded. Actual UI behavior, native signatures and render comparison not yet established. PR98 remains OPEN/DRAFT at fa2c304; do not cancel this run by advancing PR before its result is read.

## Process recovery tracer bullets

TEST-ONLY99ee882 (source branch wip/t_01a205f93f231486) was tested in native45398062674. Independently reread actual RED: `testHelperCrashBeforePairingRestartsWithoutRotatingIdentity` starts1 vs expected2. Root cause: scheduleRetry rejected every unpaired runtime, including restart=true. Integrated tests as e7caae1; minimal production32203fc permits restart without pending/persisted enrollment, retaining cancellation and generation fence.

Native GREEN candidate45409288288 is dispatched on wip/t_01a205f93f231486-crash-green at32203fc86b5ddad18110377c91d51382a847cb0e. It also includes `testClosedHelperInputThrowsWithoutTerminatingClient`, ordered by the fixture closing stdin before emitting ready. Need actual result; no speculative SIGPIPE fix yet. This branch is deliberately separate from active PR UI verification.

Next tracer bullet f02380d32da88681322f47e3efe9d615690cdc4a adds `testTransientHelperLaunchFailureRestartsWithoutRotatingIdentity`: throw once from link.start, expect automatic second startup preserving key. Native45409494369 establishes actual intended RED at line71 (starts1 vs2), while pre-pairing crash test passes1.077s. After independently reading logs, production23bf699 makes start() catch call stopLink and scheduleRetry(restart:true), preserving failure-closed identity reads and bounded backoff. Native45410096378 on wip/t_01a205f93f231486-crash-green at23bf6992acf8f0bd61759611e767673f8089199e is the newest GREEN candidate for both recovery tests and the closed-input native probe; no GREEN claim yet.

An interrupted prior run also published wip/t_01a205f93f231486-recovery at5d7a009d8f0fec3f76236008b4af888befdd0bc7 and dispatched45408123084 (queued on readback). It contains the same pre-pairing crash repair/tests as32203fc except a comment; preserve it, do not force-push or claim it lost. Push to that branch was rejected safely. Current run uses unique backup wip/t_01a205f93f231486-run125478 and the crash-green stream. No prior unknown source was overwritten.

## Focused read-only audit leads (not executed proofs)

A separate investigative session identified these app-owned paths requiring regressions before acceptance:

1. start() catch never re-armed retry; now reproduced native RED45409494369 and repaired23bf699, native GREEN candidate pending. Paired manual Reconnect no-op when running=false still merits UI recovery coverage.
2. pending invitation cannot be replaced after unavailable/wrong invite; enroll rejects pending!=nil while UI continues showing Pair router. Preserve null-invite ambiguous-success recovery, but provide explicit cancel/replacement semantics and test late responses/generation fencing rather than simply clearing pending on every offline event.
3. saveHub failure after connected calls stopLink with no restart, stranding the pending recovery path.
4. failed sign-out clears deviceID/running but retains pairing; UI hides Sign out (deviceID nil) and shows ineffective Reconnect. Keep deletion failure recoverable without silently rotating identity. Account intentionally deletes key first so stale enrollment fails closed; do not weaken that invariant just to hide error.
5. POSIX reader/stop descriptor ownership and SIGPIPE safety need native evidence. XCTest signal disposition alone may differ from shipped app; do not claim robust production behavior solely from a same-process test.

These are hypotheses grounded in source reading, not claimed regressions/GREEN. Line numbers in the investigative summary were concatenated offsets, not actual per-file locations.

## CI and remaining acceptance

At fa2c304 current PR Python45403353268 succeeds. Earlier Python45391611789@5e07553 failed duplex-audio.speech/heard-truncation step7 TimeoutError. These are different source trees, not a same-tree rerun; do not claim a quarantine-eligible flake from this alone. docs/ci-quarantine.md covers Rust/conformance IDs, explicitly not Python unit tests; any quarantine needs demonstrated same-tree flakiness and orchestrator owner, not a speculative entry.

Fresh this run: test_hub_packaging.py1 + test_dev_signing.py4 PASS; fake orchestration only, not native signature proof. git diff --check PASS. Rust helper2 real-iroh tests, strict Clippy/fmt remain historical4d8c674 evidence, not rerun this slice. Linux worker has no Swift toolchain; all Swift execution evidence is hosted.

Still required: read native UI result and download/inspect normal/large/contrast renders; run baseline presentation checker after comparison and update only reviewed changed baselines; actual universal helper/resource/inside-out signature proof; process tracer GREENs and further controller error-path regressions/fixes; independent exact-head review; all required CI green; native merge queue; finally kanban_request_review with metadata.published_pr AFTER merged. No acceptance or delivery claim yet.
