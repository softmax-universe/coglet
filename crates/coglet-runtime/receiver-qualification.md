# Runtime receiver — implementation checkpoint, NOT conformance acceptance

This branch is unfinished. No frozen case has been marked qualified by this
checkpoint. It contains no spec or wire changes, and no Factory DB/WASI edits.

## Implemented and exercised

- Actual `DeviceHandler` receives debug.read/write, hub.inbox/cancel, pause/step
  and retains their scoped operation receipts through the existing hub.receipt
  reader. Exact peer, method, actor/ticket and delegation use production Authority.
- Queued receipt + request + audit are one coordinator WAL frame. Applying
  write/inbox stages a committed `StagedBoundary {changes,runtime}` with terminal
  receipt, audit, inbox bytes and wake in one frame. Reads see materialized staged
  bytes; no seal_boundary per mutation and the journal-derived base is preserved.
- CAS, protected inbox/internal paths, byte limits, read range/UTF-8 limits,
  pending capacity and message-ID conflict/dedup. Cancel now reserves its OWN
  mutation ID and retains its original returned cancelled receipt atomically;
  changed-intent retries and cross-method collisions fail.
- Paused state persists, start resumes, and explicit step uses the actual native
  engine and durable source sequencer. Attempt reconciliation finishes the
  original step receipt; unknown attempts cannot be cancelled/re-executed.
- `hub.git.fetch` serves only operator-configured immutable, coglet-signed
  `TargetPin.fetch_snapshots` (exact ref -> {update,bundle_hex,sha256}). It verifies
  signatures, complete bundle and signed archive via the real Git validator.
  There is no default export and no automatic private-state export or signing key.
  Small backward-compatible Rust APIs in coglet-proto add a per-frame gate to
  existing fetch framing; metadata/chunks/end/terminal all recheck fresh scope.
- `hub.actions` has a REAL expired-action rejection at the receiver, retained in
  WAL before its reply and with zero dispatch/outcome. This is explicitly NOT a
  complete positive router-action bridge. Nonexpired actions remain unadmitted
  (`unavailable`), rather than fabricating adapter acceptance.

## Named tests run on this checkpoint

- `cargo test -p coglet-runtime --locked --lib receiver -- --test-threads=1`:
  9 tests passed, including durable cancel-ID retry/conflict and cross-origin
  inbox-ID content comparison, on top of the recovered WAL/recovery cases.
- `cargo test -p coglet-runtime --locked --test device_lifecycle receiver_tests
  -- --test-threads=1`: 7 default-feature tests passed. These use actual encrypted
  iroh, production DeviceHandler, real signed Git bundles and reopened WAL.
  New fetch tests exercise positive transfer, actor/ticket isolation and
  revocation after metadata/before bytes; expiry test independently reopens WAL
  and asserts no prepared dispatch or dispatch order and one retained rejection.
- `cargo test --workspace --locked -j 2`: passed. First invocation was terminated
  by the tool's 300s cap during fixture compilation; second with 600s completed.
  Workspace feature unification also executes the real native single-step test
  (43 device_lifecycle tests passed in that run). Do not count the timed-out
  invocation as a completed suite. Parsed aggregate: 830 passed, 0 failed,
  7 ignored and 105 filtered across 161 result summaries. Those ignored/filtered
  cases are not full zero-skip frozen conformance.
- `cargo clippy --workspace --locked --all-targets -j 2 -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Final separate feature-native full suite, required exact-head CI and independent
  exact-head review are NOT done. There is no PR or merge yet.

## Acceptance work still required

1. **Real in-progress guest concurrency.** Current guest execution holds the
   service mutex; back-channel step also holds Authority SQL for the full guest.
   This prevents inbox admission/cancel DURING a real in-progress step. Do not
   qualify frozen inbox-boundary-queued by a stopped-guest reply, synthesized
   queued fixture, sender refusal, or permanently unresolved fake guest.
2. Add a bounded completion/retry driver: head work after a transient failure or
   attempt reconciliation must not remain indefinitely stranded. Preactivation
   step kill-switch/manifest failures now become durable failed receipts and
   advance, but unresolved attempts still need their real completion observation.
3. Execute frozen inventory groups unchanged against actual receiver and retain
   all required reopened WAL/audit oracles (especially held guest cancel and
   256-pending backpressure). Existing tests are operational regressions, NOT
   registration of those frozen harness cases.
4. Complete positive router actions if the full producer contract requires it;
   the expiry-only path is not full hub.actions admission/outcome/recovery.
5. Coordinate installed-push integration, preserving its code_pending no-next-step
   fence and consuming staged boundary changes before a code cut.
6. Full qualification, PR, independent exact-head review, green required CI,
   merge queue, and merge SHA comments on this card and conformance owner.

Git-push cancellation remains outside this card's withdrawn Git receipt slice;
the installed-push owner explicitly leaves that refused pending separate scope.

## Next implementation recommendation (unimplemented, inspect before applying)

Prefer a two-phase device/native guest API in the SAME WAL, not a sidecar queue.
Keep the existing in-lock run_next/DurableAttempt and ABI 0.2 DB path unchanged.
Prepare inputs/candidate under service lock; run the candidate Runtime on the
same blocking thread without service or SQL locks. A custom Engine wrapper can
perform native admission, then acquire original scope SQL -> service, persist the
exact pinned unresolved_attempt/fenced_staged_prefix immediately before execute,
and release both locks. Its execute delegates to the real engine with no locks.
After the actual returned journal, publish through existing recover_attempt /
reconcile_attempt using the CURRENT live state (preserves concurrent receiver
admissions/audit/cancel), never commit the stale pre-guest ServiceState clone.
Restore native engine ownership; journal a known observation even if authority
expires during the already-entered guest. No blind rerun on unknown outcome.

Need an in-process reservation before releasing the initial service lock so a
second scheduler cannot enter before the durable fence; reject native replacement
while engine custody is taken. Defer boundary apply while that reservation/fence
exists but allow queue admission/read/cancel. Regular adapter run_native as well
as explicit debug.step must use the unlocked route; only fixing debug.step leaves
the ordinary guest lock wall. Never hold the Authority SQL callback across guest
execution. Preserve installed code_pending fence before ANY fresh guest entry.

The alternative suggested by internal design review was splitting Runtime.step
at admit/execute; the wrapper avoids that large split if its admission callback
receives fully pinned inputs and preguest reservations prevent concurrent entry.
This recommendation has NOT been implemented or independently qualified.
