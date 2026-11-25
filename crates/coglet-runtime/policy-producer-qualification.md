# ADR-0033 implementation qualification

This branch is work in progress. ADR-0033 landed docs-only in PR #167 before
runtime code. No M3 acceptance or merge-readiness is claimed here.

## Executed locally

- `cargo test --locked -p coglet-runtime --lib --features native-engine`:
  73 passed (includes native, producer-state, private local socket, and gate tests).
- `cargo test --locked -p coglet-runtime --features native-engine --test device_lifecycle policy_producers:: -- --test-threads=4`:
  all four named tests passed in parent execution; every test executes real
  cogletd/WAMR and obtains exclusive WAL custody after SIGKILL/reap, then restarts.
- Default and native-engine all-target clippy with `-D warnings` passed before
  the final database-path state-adoption edit; rerun at final qualification.

## Requirement matrix and remaining controls

| Requirement | Existing evidence | Remaining before acceptance |
|---|---|---|
| Applied generation | `attestation_matches_applied_generation_after_restart`: signed initial admission/install context, installed B, exact A commit at generation 3, byte pin, original IDs and reopen | queued-change barrier proving attestation cannot advance before code cut |
| Native health | `health_counters_bound_to_generation_and_reject_stale`: actual successful, trapping and budget-failed WAMR attempts on separately signed daemon activations, cumulative counters after reopen; stale generation/commit and forged input refusal | real daemon changed-clock/refusal, counter/order exhaustion controls; explicit reordered/replayed sample handling at consumer boundary |
| Emergency fence | `emergency_output_revoke_is_synchronous_without_router`: held exclusive SQL writer, real Unix revoke, denied subsequent adapter frame, restart closed | actual guest-generated action output control and bounded still-running guest/revoke race (a denied hello alone is not action qualification) |
| Original stop | `emergency_stop_reconciles_original_context_after_restart`: retained exact original stop ID and ticket, authenticated local reader with different ticket, authoritative receipt, no guest step during reconciliation | real failed revoke WAL write; held pending-stop authority; cross-method ID collision; unknown stop outcome |
| Local authentication/framing | `policy_local::tests`: real peer credential rejection, bad profile/binding/actor/ticket, duplicate keys/nonces, bounded frames, stale socket ownership | repeat wrong UID/principal through real configured cogletd, not only listener test; exact closed response schemas/vectors for every successful result |
| Simultaneous lease authority | same SQL snapshot now validates original start and prearmed stop; both expiry windows rechecked before lease enable | deterministic expiry and cross-method-collision negative tests for `runtime_boundary_obligation` |

## Remaining regression/release gates

Run full installed-push/device-restart/native operational regression suites,
primary ABI and both adapter-transport conformance, default/native clippy and fmt.
Do not edit frozen inputs. Ensure required Linux CI actually executes the four new
feature-native daemon tests (not zero-filtered/default-only selection). Review all
new state validation, unknown-outcome and publication concurrency independently.
Only then publish the code PR, obtain exact-head independent review and required
CI, enqueue with `--merge --match-head-commit`, and wait for verified MERGED.

Opt-in producer profiles gate only the router-service adapter connection owned by
DeviceHandler. Standalone legacy `--surface-service-socket` and C ABI have no
producer-profile provisioning and cannot claim ADR-0033 safety qualification.
