# PR231 required-CI race repair (not stage2 qualification)

The exact reviewed head `40f95304339c89387321f3d2168260bec99ad1f4`
failed `installed_push_crash_reconciles_exactly_once` in native `rust-core`
job140938694872, run46131375899. Aggregate rust job140957426680 also failed.
No enqueue or hosted qualification was performed on that red head.

Full public job logs contained complete before/after WAL arrays. Independent
read-only diagnosis sa-0-aad6a7bf decoded them with existing locked sim cbor2;
the executor reran the complete semantic-diff assertions. Before:38689bytes,
3frames. After:55621bytes,4frames, exact before-prefix retained. The appended
frame changed only `managed.code_pending` true→false plus hashchain metadata;
journal remained null, operation receipts unchanged, code-revision segments1→1.
This is legitimate fence cleanup, not a duplicate code-update effect.

`finish_installed_pushes` synchronizes the fence on the iteration AFTER the
terminal SQL receipt becomes visible. The old test used receipt visibility as
its WAL baseline barrier. Runtime and this test were unchanged against main.
No same-configuration unmodified-tree passing rerun was used to waive this red.

## Narrow plan and retained coverage

Plan review sa-0-3e0fc804 APPROVE required rejecting partial copied tails and
returning exactly the validated capture, rather than rereading live WAL afterward.
Only tests change. Existing DurableStore validates a disposable copied WAL;
recovery truncation must not shorten it, and code_pending must be exactly false.
The timeout remains bounded15s; the original raw-byte equality after retry,
terminal receipt and single-segment checks remain unchanged. No runtime ordering,
effects, frozen contract, source-closure policy, scheduler, or CI workflow changes.
Reversal: revert this test-only commit, restoring the diagnosed test race.

Pre-edit inventory: existing76 device_lifecycle tests and137 stage2 fast cases.
Added separate two-test `wal_snapshot` target. Existing tests not removed/skipped.
Initial regression invocation was blocked by missing cmake (not RED evidence).
User-space `uv run --no-project --with cmake --with ninja` supplied build tools;
then regression compile genuinely failed missing support/wal_snapshot.rs before
implementation. This is missing-module RED, not a claimed deterministic original
CI-race reproduction. Complete CI arrays provide the original observed failure.

## Real local results

- `cargo test --locked -p coglet-runtime --test wal_snapshot`:2PASS after helper.
  Tests cover pending/missing/null/nonboolean refusal, exact settled capture,
  partial headers and bodies after a settled frame, bad checksum, permanent-fence
  timeout and successful exact capture while the original WAL remains locked.
- `cargo test --locked -p coglet-runtime --features native-engine --test device_lifecycle installed_push_crash_reconciles_exactly_once -- --exact --test-threads=1`:1PASS,68.06s.
- Same command selecting
  `hub_receipt_cogletd_sigkill_recovers_original_scope_and_no_duplicate_install`:
  1PASS (the shared-helper sibling remains unchanged).
- `cargo clippy --locked -p coglet-runtime --features native-engine --test wal_snapshot --test device_lifecycle -- -D warnings`:EXIT0.
- `cargo fmt --all -- --check` and `git diff --check`:EXIT0.
- `uv run --project sim --locked python -m unittest discover -s tests/unforged_proof -q`:137PASS.

Cargo invocations used uv-supplied cmake/ninja. Full exact-head hosted CI,
independent code review, native queue/main merge and containing-main seven-case
hosted proof remain required. No stage2 PASS or historical-green waiver.
