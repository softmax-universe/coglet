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

## Qualification recovery and driver tranche

The detached checkpoint `ab394c9` full-workspace chain exited 101 in
`managed_lifecycle::native_scheduler_obeys_durable_stop_and_resumes_only_after_explicit_start`.
The remaining chain did not run. Isolated reproduction failed identically:
synthetic adapter times preceded initialization of the newly resampled durable
clock. The test now uses `supervised_now` consistently, retaining fresh fence
checks rather than relaxing production clock/authority rules. Managed lifecycle
9/9 then passed. After the driver and idle-boundary changes, the native encrypted
receiver group passed 10/10, native all-target clippy `-D warnings` passed, and
fmt/diff checks passed. Initial new ordinary-guest test attempts did not enter a
guest: one used an unsubscribed external event, then one omitted the actual
adapter pump. The corrected test uses an admitted inbox wake and explicit real
`pump_dispatch`, observes the unresolved fence while still running and retains
the original bounded guest observation. Neither failed fixture attempt counts
as execution proof. Full new-head qualification remains required.

The driver checkpoint `d8467fb` workspace suite completed (834 passed, 0 failed,
7 ignored, 105 filtered across 161 summaries). Its separate native suite was
intentionally interrupted during device-lifecycle tests after a tranche review
found that the idle apply path incorrectly reused the expired RPC deadline as
application authority. The new deterministic fault-seam test first reproduced a
retained failed receipt after admission; the fix applies with original authority
and path checks only, leaving the reply deadline distinct. The new native
receiver group passes 11/11 including both inbox/write timeout cases and reopened
applied receipts/files. Strict native all-target clippy and fmt/diff pass. The
interrupted native suite is not whole-suite proof, and the review is not final
ship approval.

## Acceptance work still required

1. **Real in-progress guest concurrency now implemented, frozen harness pending.**
   `detached.rs` releases SQL/service locks around the actual guest for normal
   adapter pumping and debug.step. Exact preguest reservation is fenced in the
   same WAL, and the original worker observation reconciles against current
   state. The real bounded-WAMR encrypted-iroh admission/cancel test passes.
   Internal review found pre-fence staged publication loss, stale surface epoch
   authority and nonatomic engine/reservation restore; all three were corrected.
   New coordinator tests preserve a concurrent staged inbox across preguest
   refusal/reopen and ensure a live authority gate refuses before the fence and
   prevents another entry until original caller restore. Native receiver unit
   group11, wire group9 and native_operational36 tests pass after these fixes.
   The whole native suite passed before the three review fixes, not after them;
   run final whole-suite qualification on the published exact head.
   None of this registers/qualifies the unchanged frozen case names yet.
2. **Bounded receiver completion/retry driver implemented.** A runtime-owned weak
   driver retries after 250ms with exponential transient-error backoff capped at
   5s. Target-local async permits serialize all drains; custody stays with the
   blocking worker even if its awaiting future is cancelled. A batch handles at
   most 256 operations, and unresolved attempts remain unselected until their
   original observation is reconciled. An actual ordinary adapter/WAMR guest
   regression admits an inbox while the guest runs, sends no further network
   request, waits for automatic materialization, then reopens receipt/file state.
   Idle head inbox/debug writes now materialize synchronously under original
   authority, returning the actual applied receipt; in-progress work still queues.
   This is not yet the frozen named fixture/restart qualification.
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

## Execution architecture (implemented; complete final qualification still required)

The two-phase device/native guest API below is now in `detached.rs`. Keep the
existing in-lock run_next/DurableAttempt and ABI 0.2 DB path unchanged.
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

The wrapper pins the original base tree, manifest and exact staged change list;
a concurrent preguest publication refuses entry without losing that publication.
The original caller restores engine/debug ownership and releases its reservation
under one lock. Surface entry rechecks original epoch and fresh supervised grants.
These internal review fixes still need final whole-head qualification and review.
