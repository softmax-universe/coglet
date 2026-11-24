# Hub enrollment implementation notes

## Contract and boundaries

- Frozen `spec-v0:spec/hub-protocol.md`: pinned hub identity, ALPN `coglet/hub/0`, peer-bound 256-bit invite, fresh enrollment on each connection, null-invite recovery after ambiguous success. Short-code lookup and QR encoding are not wire methods.
- Shell/protocol/kernel/mock part-1 parent receipts recovered as DONE (PR12/48/58/74).
- Configure surface: preserve Settings sidebar, heading, search and DesignGroup. Replace only Hub placeholder with shared enrollment controls. Preserve onboarding steps, permissions/login controls and footer. Reuse existing tokens and keyboard navigation. Menu connection availability remains independent of voice/activity.
- No UI implementation or visual acceptance yet. Baseline gate and native rendered comparison remain mandatory.

## Implemented, partial

`HubTransport/` is a standalone Cargo package. Production helper uses landed coglet-proto Session + real iroh. Key/config arrive only through anonymous pipes, not argv/env/files/logs. Output is sanitized state/public device ID. Commands bounded to16KiB. Null-invite recovery precedes initial invitation; connected only after version selection; QUIC closure yields offline; EOF closes endpoint.

Swift HubAccount persists a32-byte generated-once seed before use, fails closed on Keychain errors and never silently rotates it. HubKeychain injection tests use memory backend; Security backend uses generic password and `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`. Non-secret hub config uses UserDefaults; invite is memory-only. HubConnection provides cancelable, generation-fenced reconnect with30s inclusive jitter cap, and local sign-out deletion. ProcessHubLink bridges anonymous pipes and fences stale callbacks.

Native run45341075790 and run45342465981 compiled production Swift and passed four HubConnectionTests +two HubKeychainTests. Optional native mock test skipped without executable paths; NOT acceptance. Required real-helper/native test still unexecuted.

Rust real MockHub enrollment/drop/offline/recovery/revocation and private-stdio/EOF tests passed locally; strict all-target Clippy/fmt passed on6071ef2. Forward main integration e9e5817 and regenerated standalone helper lock repaired actual transitive reqwest lock conflict.

## Native dependency repair integrated; acceptance pending

The Darwin compilation failure in run45342465981 is repaired by merged PR101 (parent `t_01a20c2f56d0d2ad`, DONE). Its reviewed repair fcc0490 was integrated as53e2e68. A fresh GitHub DIRTY result and merge-tree reproduced real workflow conflicts; forward merge82ff3ca integrates main6efc352, preserving both app acceptance and Darwin refusal checks plus the newer iOS workflow. No validator enforcement was weakened: non-Linux subprocess validation explicitly refuses before spawn; Linux retains unreaped-leader custody.

PR98 remains draft at82ff3cae935a8928b88008f4cecb3595ba8d2c67; backup43f7f33 adds checkpoint documentation only. Native run45363249974 FAILED: four HubConnectionTests and two HubKeychainTests passed, and the Darwin proto check/refusal test passed. The real mock-router launcher build then failed at sdk/surface-rust/src/ids.rs:43 because libc::getrandom is Linux-only and absent on Darwin. Current main6efc352 has the same implementation. The required real-helper/mock test was SKIPPED; optional test skipping earlier is not acceptance. Repair owner t_01a20ce01825149f (alpha-follow, running) is linked as a prerequisite, preserving the portable CSPRNG/error contract rather than weakening the native lane. After it merges, integrate its reviewed repair and rerun the actual native test. Both real-iroh helper integration tests, strict all-target Clippy/fmt and five portable packaging/signing cases passed in the previous run; they were not reexecuted during this failure diagnosis. Settings/onboarding/menu code remains untouched.

## Packaging slice (portable orchestration verified only)

- `scripts/build-hub-helper.sh` builds --locked --release for aarch64/x86_64 Apple targets, lipo-merges and verifies both architectures, atomically installs executable under Contents/Helpers/softmax-hub-link.
- project.yml post-build invokes that script. Rust toolchain/targets are required for Xcode builds.
- sign-dev.sh signs the helper before outer app. Existing release path recursively signs Mach-O.
- Tests/test_hub_packaging.py uses explicitly fake cargo/lipo to verify orchestration and paths with spaces; Tests/test_dev_signing.py uses fake signing/keychain to assert inside-out order. Both observed expected RED, then GREEN (1 packaging +4 signing cases). These are NOT native universal binary, codesign or TCC proof. Native build remains blocked above.

## Remaining delivery

1. Obtain dependency repair and real native helper/mock acceptance; read actual test output, not just job status. Required workflow supplies executable paths and SOFTMAX_REQUIRE_HUB_TESTS=1, so it cannot count a skip as success.
2. UI wiring: local pasted pairing JSON/code, QR image import, pinned manual developer hub/address, public device ID copy, hub name/status and local unenroll. Envelope fields name/hub/addresses plus optional relay/invite/device; validation already in HubPairing. Do not invent remote short-code lookup.
3. Wire ShellModel lifecycle and existing menu availability; shared controls only in existing Settings Hub and onboarding placeholders. Run baseline gate before UI edits and real native render comparison after.
4. Verify native Xcode helper bundle architecture, resource lookup and inside-out signing on actual target. Portable fixture tests do not satisfy this.
5. Revisit ProcessHubLink pipe-write SIGPIPE and crash/restart coverage.
6. Independent exact-head review, every required check green, native merge queue, then kanban_request_review with published_pr. PR98 remains draft; no merge/delivery claim.

## TDD provenance

Rust initial library RED then real-iroh GREEN. Swift initial missing core API RED run45337370014; missing process bridge RED run45339321655; missing Keychain injection RED run45339491432. Production Swift six-test GREEN above. Linux worker has no Swift toolchain: native results are hosted evidence, never local execution.
