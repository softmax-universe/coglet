# ADR-0033 implementation qualification

This branch is work in progress. ADR-0033 landed docs-only in PR #167 before
runtime code. No M3 acceptance or merge-readiness is claimed here.

## Executed locally

- Parent executed `cargo test --locked -p coglet-runtime --lib --features native-engine`:
  78 passed, including reply schema/vector and nonreading-peer gate controls.
- Parent executed `cargo test --locked -p coglet-proto --lib authority::runtime_boundary::tests:: -- --test-threads=1`:
  6 passed (both-method expiry, revocation, collision and same-writer custody).
- Separate implementation session executed all nine original `policy_producers::` tests
  serial: 9 passed in 632.95s (before the four controls below). These are actual cogletd/WAMR, SIGKILL/reap,
  exclusive WAL reopen and restart cases, not in-process producer substitutes.
- After replacing blocking publication with a single nonblocking send, parent
  independently reran `emergency_output_revoke_is_synchronous_without_router`,
  `guest_action_output_races_revoke_without_exposure_after_return`, and
  `failed_revoke_wal_write_leaves_original_stop_unresolved_until_reconciled`:
  each passed against actual cogletd/WAMR and reopened WAL.
- `check-primary-abi.py`: 21 primary ABI cases passed (subset only).
- `check-surface.py`: native pointer/buffer/malformed probes plus 2 negotiation
  cases over each native transport passed; not full operational qualification.
- Default and native-engine all-target runtime clippy `-D warnings`, fmt and diff
  checks passed (rerun at final head).

## Requirement matrix and remaining controls

| Requirement | Named evidence | Remaining before acceptance |
|---|---|---|
| Applied generation | `attestation_matches_applied_generation_after_restart`: signed initial admission/install context, installed B, exact A commit at generation 3, byte pin, original IDs and reopen; `queued_code_change_cannot_advance_attestation_before_cut`: actual validator barrier and queued receipt, unchanged A producers until released cut | final regression/review |
| Native health | `health_counters_bound_to_generation_and_reject_stale`: actual successful, trapping and budget-failed WAMR attempts, durable counters and stale/forged refusal; `changed_clock_and_overflow_fail_closed_after_real_wal_reopen`: real daemon CLOCK_BOOTTIME rollback and conversion overflow refuse samples/lease | `order_exhaustion_after_real_wal_reopen_refuses_without_wrapping`: after SIGKILL/reap a legitimate near-limit state frame (`observation_sequence = u64::MAX-1`) is committed through the real `DurableStore`; the restarted daemon admits it, returns sequence `u64::MAX`, then `resource_exhausted` on every later sample (also after a second real reopen), WAL value stays `u64::MAX`, no guest entered. `forged_near_limit_counters_are_refused_by_real_wal_recovery`: counters are recomputed from retained journal records, so a seeded `attempted = u64::MAX` is refused at recovery (cogletd exits 1, `recovered policy counters differ from actual boundaries`). **Counter exhaustion through a real daemon is not reachable** without ~2^64 retained journal records and stays covered only by unit `policy_counter_exhaustion_refuses_before_entry`; this is a boundary, not a pass. Seeding uses the WAL's own frame format after the daemon is dead; it is test-only state construction, not production behavior. Consumer replay/order acceptance belongs to C, not this producer |
| Emergency fence | `emergency_output_revoke_is_synchronous_without_router`: real guest act received before revoke, held SQL writer, drained revoke and no new socket bytes; `guest_action_output_races_revoke_without_exposure_after_return`: concurrent continuing real guest and stable receive queue; `nonreading_peer_cannot_hold_revoke_or_expose_after_return`: actual socket backpressure | full regression/exact-head review; race is load-based, not deterministic mid-syscall injection |
| Original stop | `emergency_stop_reconciles_original_context_after_restart`: retained exact stop ID/ticket, held SQL authority refusal, wrong actor/ticket, authoritative receipt, idempotency and no guest entry during reconciliation; `failed_revoke_wal_write_leaves_original_stop_unresolved_until_reconciled`: exact-WAL ENOSPC shim, sticky refusal, restart prearmed obligation and original receipt | `stop_applied_but_custody_unrecorded_reconciles_once_and_refuses_id_reuse`: real `hub.stop` receipt applied under the original ID, daemon SIGKILLed before custody recorded Stopped; after restart a new lease is refused, reconcile returns the original ID, the receipt is unchanged (no second effect), reconcile is idempotent, other-ticket reuse of the ID is refused, no guest step ran. Not covered: a real daemon killed *inside* a native guest (unresolved attempt + queued stop): WAMR budgets are compute-metered so a guest cannot be held deterministically; that fence stays covered by the fixture-engine unit `policy_wal_coordinator_keeps_unknown_and_reconciles_once` only. Torn-tail shape is still not asserted (current ENOSPC test proves injected failure, not tail shape) |
| Local authentication/framing | `policy_local::tests` and `configured_uid_principal_and_stop_id_collision_are_refused_by_real_daemon`: configured wrong UID, wrong principal, actor/ticket, closed/duplicate/oversized frames and real daemon refusal; closed reply schema/vectors in tests/vectors plus 4 reply tests; actual RuntimeService attest/health serialized results checked against schema | `all_five_daemon_methods_return_shared_schema_conforming_results`: `Daemon::raw` validates EVERY actual daemon reply in all 13 producer tests (success and failure) against `tests/vectors/policy_local_replies.schema.json` with a verbatim copy of the strict unit-test validator (`tests/support/policy_reply_schema.rs`), plus envelope echo, request pins, daemon-instance and counter reconciliation; this test requires a validated success for all five methods and rejects mutated real replies. Not covered: the full semantic checks of `policy_local_reply_tests::semantic` (only the subset above) |
| Simultaneous lease authority | `authority::runtime_boundary::tests::`: six deterministic tests validate both methods in one SQL write snapshot, all expiry windows/final clock and cross-method Git ID collisions; daemon rejects install/code IDs as stop IDs | additional actual start-ID collision control if needed by final review |

## Publication boundary

The opt-in adapter path constructs the frame outside the output mutex, then does
exactly one `send(MSG_DONTWAIT | MSG_NOSIGNAL)` under output custody. Backpressure,
errors or a short send terminate that connection; the prepared durable dispatch is
not relabeled physically successful. Revoke closes the state first, then drains
that one syscall; it never times out and returns while an in-flight writer can
still expose bytes. No router SQL, guest, runtime mutex, peer read or retry loop is
inside the publication critical section. An already received/accepted physical
output is not cancelled. Standalone legacy adapter and C ABI have no producer
profile and cannot claim ADR-0033 safety qualification.

## Remaining regression/release gates

Run full installed-push/device-restart/native operational regression suites and
workspace tests; preserve frozen inputs. Required Linux CI already invokes
`scripts/ci/cargo-test --locked -p coglet-runtime --features native-engine --test device_lifecycle`
with `RUST_TEST_THREADS=1`, which includes all thirteen producer tests; no workflow edit
or new shared CI surface is needed. Verify actual executed counts at exact head.

Then obtain independent separate-session exact-head review, publish the code PR,
verify every required check, enqueue with `--merge --match-head-commit`, and wait
for verified MERGED. Implementation children are not final independent review.
