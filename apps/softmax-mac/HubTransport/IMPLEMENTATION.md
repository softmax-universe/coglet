# ROUTER enrollment implementation notes

## Current continuation authority (supersedes historical notes below)

### Run132517 update

PR98 is OPEN/DRAFT at1541a24fab8f0746e496ce0e3582472f5e7f2aff. Independently verified native45450382089 SUCCESS with supported exact --once exit0 and read raw log: failed-signout, persistence and replacement account tests, real mock enrollment/drop/recovery, UI enrollment, universal helper/app valid-on-disk signatures and snapshot errors[] all pass. This supersedes the prior pending native result, NOT final acceptance.

Independent separate-session review sa-0-bd9bd951 returned REQUEST_CHANGES: startup identity errors leave deviceID nil and hide Sign out, blocking explicit reset when enrolled key is missing. Source independently rechecked. New f8df2bade5f2c31c8ca6a19ca0faccd482ac6611 on wip/t_01a205f93f231486-reset-red adds testStartupIdentityFailureKeepsSignOutReachableWithoutRotatingKey. The original visibility condition deviceID!=nil is extracted unchanged into canSignOut, consumed by UI. No behavior repair yet. Exact native RED45458930583 dispatched/readback; require actual XCTest false-versus-true failure, not compiler/setup error, before changing canSignOut to include error/local enrollment state. Failed cleanup must keep action reachable; successful cleanup must hide it. PR98 deliberately unchanged to preserve native GREEN.

Other reviewer leads remain pending: detached ProcessHubLink reader must not share a descriptor closed/reused by stop(); generation fences cannot restore stolen pipe bytes. Native stop/restart regression and reader-owned descriptor correction needed. Also Apple TN3137 confirms SecItem defaults to file-based keychain; setting accessibility alone does not select Data Protection. Investigate/use kSecUseDataProtectionKeychain with synchronizable=false, preserving signed-app entitlement/provisioning and fake tests; no OS Keychain enforcement proof yet. No live Keychain touched.

Rust45450382196 and dependency mirror45450382001 fail lifecycle_operation_retries_cannot_reverse_later_transitions_after_restart at1489 (Null versus original applied receipt). Existing PR136 explicitly owns that WAL restart race; downstream notification independently read back at issuecomment7140593265. One failed-only Rust rerun requested (attempt2). No runtime code/quarantine/waiver added. Python45450381951 attempt2 passes; docs explicitly exclude Python units from quarantine machinery, so no unsupported entry. Main rust45448563243 is green. All five original parent DONE receipts recovered again via authorized replicas; no dependency edits/created cards/worktrees/processes.

Fresh packaging1 + signing4 and diff-check PASS at f8df2ba. NEXT read exact reset RED45458930583, minimally repair predicate only after observed RED, verify GREEN, then close fd/Keychain review leads, final exact-head independent approval/all CI/merge queue and request_review AFTER merged. Review notes/raw logs retained in task workspace; native run URLs authoritative. No acceptance or landing claim.

### Run130521 update

Independently read completed native logs, saved in workspace/persistence-45423268341.log and replace-45433755546.log. Persistence candidate45423268341@5e6d633 PASSES testEnrollmentSaveFailureRestartsChannelAndRecoversSameIdentity (1.198s), required real mock lane, portable core, universal helper/signatures and reviewed snapshot comparison. Overall FAILURE is testSettingsFocusedSearchArrowNavigation (SoftmaxUITests.swift214/217/220/227): four focused-search selection assertions. Do not call it full GREEN or rerun/waive without diagnosis and quarantine-policy disposition. This UI navigation code/test is unchanged by persistence repair; causal/inherited/flake attribution is not yet proved.

Replacement RED45433755546@6766f2c is the intended thrown pairing error at HubConnection.swift52 in testOfflinePendingInvitationCanBeReplacedWithoutAcceptingOldCallbacks; persistence recovery also passes1.241s there. Applied minimal ef36558fe0c5bee71dd9b2b1b720aaa0baaf454a: explicit offline pending replacement cancels retry, stops/increments original channel generation, retains key, replaces pending envelope and restarts. Active/enrolled replacement stays forbidden. Candidate45443085385 dispatched on wip/t_01a205f93f231486-replace-green, exact head read back; supported --once exits2 queued. No native GREEN claimed. Fresh packaging1 + signing4 + diff-check PASS.

All five original prerequisite DONE replicas reread. PR98 remains OPEN/DRAFT b2e72d8; new source is deliberately separate pending native proof. NEXT: exact --once run45443085385 at ef36558fe0c5bee71dd9b2b1b720aaa0baaf454a with macos=success; inspect replacement/account/mock/UI/signing/snapshot results. Resolve the actual focused-search failure if it recurs, never label a flake without evidence. Then failed-signout tracer bullet (current signOut clears deviceID before fallible deletion; HubSettings only offers retry when deviceID exists), preserving failed cleanup reachability and callback fencing. Final integration onto PR98, independent exact-head review/all CI/native queue/merge/request_review still pending. No local background processes, new worktrees or cards.

### Run128614 update

Native SIGPIPE candidate45417062630 at exactb2e72d84776622d553f33c024c457b6d78971934 is SUCCESS (supported --once exit0). Independently read full raw log workspace/sigpipe-green-45417062630.log: closed-input test PASS0.041s; real mock-ROUTER enrollment/drop/offline/recovery PASS1.692s; dedicated native lane3 tests/0skips. UI Pair router smoke PASS11.647s. Universal helper and inside-out ad-hoc signature verification succeed (app/helper valid on disk and designated requirements satisfied). Actual native snapshot matrix360/360 errors[]; earlier missing-render errors in the same log belong to intentionally failing checker unit fixtures. All job steps succeeded. This is not notarization or TCC proof.

Persistence GREEN candidate45423268341 at5e6d6332884ca554cb7c6932ca95d6d6205a22eb is now executing (job138548269169 started16:06:16Z); latest --once returns2. Account step succeeded, real helper/mock build in progress. Do not infer final GREEN yet. PR98 remains OPEN/DRAFT b2e72d8 until actual persistence result is read, then integrate test+repair.

Next TEST-ONLY tracer6766f2c9d53ec9f4140b2807bf44439d60ee7bb5 on wip/t_01a205f93f231486-replace-red adds testOfflinePendingInvitationCanBeReplacedWithoutAcceptingOldCallbacks. It keeps active/enrolled replacement forbidden, permits explicit offline replacement with same identity and requires old-channel callback fencing. Production is UNCHANGED pending actual RED. Exact native45433755546 dispatched and readback verified; --once with macos=failure returns2 (queued). Expected RED is thrown HubError.pairing at the offline replacement enroll call, not a compiler/setup failure. After actual RED, minimally support offline pending replacement by restarting/fencing the channel before the new target connects; preserve null-invite-first recovery and no implicit replacement of persisted enrollment. Native GREEN then required.

All five original parent DONE receipts reread via authorized replicas PR48/58/12/74/101. Source begins at verified backup292b850 and new regression6766f2c; do not restore older PR-only/wip source. No local background jobs, added worktrees or owner cards this run. Still pending: persistence GREEN/integration; pending replacement RED→GREEN; failed-signout recovery coverage; final independent exact-head review/all CI/merge queue/merged request_review. No acceptance or landing claim.

### Run126829 update

Requested native job138526753645 actually ran on GitHub Actions1000815242: the earlier macos-latest shared capacity wall is no longer an established outage. Raw job log saved as workspace/save-red-45417257441-full.log. At exact611fdc3, testEnrollmentSaveFailureRestartsChannelAndRecoversSameIdentity establishes intended RED: starts1 vs2 at line94 and offline vsconnected at101. Other account tests pass; early optional native test skip is not required mock acceptance.

Minimal production repair5e6d6332884ca554cb7c6932ca95d6d6205a22eb adds scheduleRetry(restart:true) after save-failure stopLink. Pending invitation, identity and generation fences are unchanged. Pushed/readback wip/t_01a205f93f231486-save-green; dispatched exact native45423268341. Supported --once at15:15Z returns2 (queued). No Swift GREEN claim. Fresh portable packaging1/signing4 and diff-check pass; Rust tests not rerun this slice.

PR98 deliberately remains OPEN/DRAFT b2e72d8 to preserve independent SIGPIPE native45417062630, still queued on supported --once at15:15Z. Do not advance it before reading that actual closed-input test and full mock/UI/signature/snapshot results. Latest local source includes persistence test+repair and documentation, not just PR head. Once native45423268341 proves recovery GREEN, integrate source onto PR, then handle remaining pending-invitation/failed-signout leads regression-first. Final independent exact-head review/all CI/native queue/merge/request_review remain required; no acceptance or landing claim.

PR98 OPEN/DRAFT head b2e72d84776622d553f33c024c457b6d78971934. All six prerequisite DONE receipts reread from authorized replicas (PR12/48/58/74/101/114).

Native45403353278@fa2c304 passed functional account, real mock enrollment/drop/recovery, UI Pair router smoke, universal helper and inside-out signature verification, and render matrix. Only snapshot comparison failed. Prior continuation inspected normal/large/contrast changed renders and updated92 baselines in d5e6175; snapshot-comparison GREEN still pending.

Native45411903969@d5e6175 is a NEW actual SIGPIPE RED: testClosedHelperInputThrowsWithoutTerminatingClient kills xctest with unexpected signal13. Account crash/launch-retry tests succeeded before that step. Production b2e72d8 sets Darwin F_SETNOSIGPIPE on the command pipe descriptor before helper launch; no process-global signal disposition change. Native45417062630 is the GREEN candidate at exact b2e72d8 (queued on last supported --once check); must read actual closed-input test, full mock integration and later UI/signature/snapshot results.

Separate RED-only tracer611fdc3cc9bdff985f76386b224f10df739f12b3 on wip/t_01a205f93f231486-save-red adds testEnrollmentSaveFailureRestartsChannelAndRecoversSameIdentity plus one fault seam in MemoryHubStore. Native45417257441 dispatched/readback exact611fdc3. No production persistence-recovery fix yet. Expected RED: starts1 rather than2 after saveHub failure; compiler/unrelated failures do not establish this RED. After confirmed RED, add scheduleRetry(restart:true) after stopLink in the save failure catch, preserving pending invitation for null-invite-first recovery and generation fencing. Test also rejects stale callback and checks same identity/recovered persistence.

Local clone migrated with older5d7a009; restored PR d5e6175 before edits. Current source includes both streams; do not restore old wip over it. Fresh Linux at611fdc3: two real-iroh Rust helper integration tests PASS, strict all-target Clippy/fmt PASS, portable packaging1+signing4 PASS, diff-check PASS. No Swift toolchain here; native proofs remain hosted.

Still pending: SIGPIPE GREEN; persistence recovery RED→GREEN; pending invitation replacement and failed-signout UI recovery leads below; final exact-head independent review/all CI/merge queue; then kanban_request_review with published_pr AFTER merge. No acceptance or landing claim.

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
