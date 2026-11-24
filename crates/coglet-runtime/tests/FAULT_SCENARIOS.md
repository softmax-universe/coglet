# Deterministic recovery fault scenarios

Run `cargo test --locked -p coglet-runtime --test fault_scenarios`.
The existing workspace Rust CI discovers this integration test automatically;
no production feature, workflow modification or frozen fixture edit is required.
The scheduler uses fixed seed `0x00C0_61E7`, explicit logical microsecond delivery
times, and at most three send attempts. It never sleeps or chooses a production
retry policy. Router integration uses real localhost iroh connections, with a
30-second overall deadline and 1-second operation timeouts.

## Requirement → named test

Names below are in `fault_scenarios` (router and disk tests include module prefixes).

| Fault / requirement | Named test | Observable recovery oracle |
|---|---|---|
| Drop, delay, reorder; finite retry budget | `fixed_seed_schedule_reorders_delays_and_bounds_retries` | Known `[3,2,1]` delivery schedule; nothing before due time; three drops terminate; fourth send is refused |
| Lost adapter outcome reply | `adapter_lost_terminal_reply_restart_replays_receipt_not_effect` | Durable Rust adapter receipt survives reopen; same terminal payload/source; backend invocation count stays one |
| Adapter disconnect with in-flight message | `adapter_delayed_action_after_disconnect_cannot_execute` | Disconnected link discards old pending delivery; zero effects until a fresh admitted request after reopen |
| Adapter process-loss seam with uncertain effect | `adapter_crash_with_pending_effect_recovers_unknown_without_reexecution` | Drop core without disconnect callback; reopen pending ledger, retry returns failed/unavailable, backend invocation count remains one |
| Actual runtime process exit mid-step | `runtime_process_crash_fences_attempt_and_reconciles_original_observation` | Child exits 73 after uncommitted write; real WAL retains queued batch/fence, no record or guest write; three reopen execution requests never enter guest; fixture-only reconciliation then independent scripted replay returns original output hash |
| Torn runtime result commit | `runtime_torn_commit_recovers_last_complete_state_hash_at_header_payload_and_tail` | Tear actual result frame at eight header/payload/tail boundaries; previous attempt fence survives, partial files/actions/journal never commit; complete frame restores exact record and independent scripted replay output hash |
| Disk full after guest return | `disk_full_fault::runtime_disk_full_during_result_commit_never_dispatches_or_reexecutes` | Linux child injects 7-byte short WAL write followed by ENOSPC; failed commit publishes no record, handle refuses further execution; reopen preserves queued attempt/fence |
| Router reorder, lost reply, disconnect/restart, retained historical dedupe | `router_faults::router_iroh_lost_reply_reorder_disconnect_restart_retains_old_batch_receipts` | Offset 1 before 0 conflicts without admission; dropped reply after durable offset-0 admission; ack delivery, admit/ack offset 1, close QUIC, reopen router/re-enroll; identical offset-0 retry with fresh correlation returns original offset 1 without redelivery; changed payload conflicts; offset-1 receipt also survives |

`crash_mid_step_child` and `disk_full_fault::disk_full_child` are subprocess entry
points, not additional scenarios. Parents assert the actual exit code or an
executed marker; a filtered zero-test subprocess cannot count as proof. On
non-Linux platforms the ENOSPC scenario is not compiled; portable scenarios
still run. Linux Rust CI provides `cc` for the small test-only preload shim.

## Scope and safety boundaries

- Message-boundary drops/delays/reordering are injected above the real protocol
  implementations, not into encrypted QUIC packets. Router requests still cross
  real iroh operation streams and actual durable kernel admission. A lost reply
  is discarded by the scenario driver *after* that admission completes.
- Runtime tests use the explicit scripted Engine seam, not compiled WAMR.
  They independently replay the supplied script with committed file writes and
  staged actions; this is not cross-engine or real guest qualification. M1's
  separate compiled-echo/WAMR integration remains the native integration proof.
- The crashed guest's unknown outcome is **not** declared successful. Its test
  supplies a separate known fixture observation solely to exercise the privileged
  reconciliation API; it is not evidence that this process actually completed.
  Production reconciliation still requires caller-authenticated original evidence.
- Adapter fault scenarios exercise the real Rust SDK ledger with a scripted
  native backend. The pending-adapter crash test models loss of core memory by
  dropping without disconnect; only the runtime crash uses actual process exit.
- No exactly-once physical-effect guarantee is asserted. Duplicate requests do
  not reinvoke this test backend; unknown provider outcomes remain unknown.
  Outcome receipt payloads are stable, while reconnect event sequence/time may
  legitimately change. Retry authorization is never inferred from `retryable`.
- ENOSPC is a Linux subprocess-local libc `write` interposition scoped to an
  exact disposable WAL pathname and armed only after the guest returns. It
  neither fills the host filesystem nor requires a mount or elevated privilege.
  It tests write exhaustion/short result frames, not sync failure or real power loss.
- No WAL is manually rewritten into a forged valid transaction. Torn-frame
  tests copy actual runtime commits and truncate them. Both intact and partial
  result cases are tested; the committed script changes `/data/count` and stages
  an action so hash equality cannot be established by an unchanged empty step.

Spec basis: `spec/abi.md` local atomic commit, failed-batch/unknown-attempt
fencing and replay; `spec/surface-protocol.md` durable action identity and
unknown-effect recovery; `spec/hub-protocol.md` relay offsets, lifetime receipt
retention, authenticated reconnect and bounded operations. This suite adds no
protocol, timeout, retry-authorization or retention policy.
