# Daemon restart admission (ADR-0027)

`cogletd --router-service` awaits `DeviceHandler::recover_running` before binding
its iroh endpoint or enrolling/advertising device readiness. Installed running
intent is not authorization: recovery revalidates the signed **installed** audit
bundle (not the newest staged ref), recovered code/manifest, original start
operation/intent/scope, exact retained/config/current generations, current SQL
scope/grant/delegation and trusted time, and local kill state. Native enable does
not enter Wasm; the adapter pump is started only after the admission decision.
Time and kill are checked again after native engine initialization.

Admission refusal with an intact WAL and identity commits `faulted`, disables
native scheduling, and retains every historical receipt. Original start retries
cannot clear it; a fresh distinct authorized start can atomically replace the
recovery context. An unreadable/invalid identity or WAL, or failed fault commit,
returns a startup error instead of advertising readiness. The stored error is a
bounded generic restart refusal, not a diagnostic disclosure of authority data.

## Requirement to named test

All commands are from the repository root; `--locked` is required.

| Requirement | Named test / command | Boundary |
|---|---|---|
| Actual Wasm executes again without a fresh start | `running_coglet_resumes_execution_after_daemon_restart` | Linux actual cogletd SIGKILL/restart, real socket speech event and durable post-restart committed records, WAMR compute > 0 |
| Tampered installed archive refuses execution | `restart_revalidation_refuses_tampered_artifact` | Actual daemon restart; faulted status and no new guest records |
| Current authority, expiry/revocation, both directions of generation mismatch | `restart_admission::restart_revalidation_refuses_expired_or_changed_authority` | Handler restart; includes clock advancing at pre-enable and post-initialization gates, after expensive artifact/signature validation |
| Latched or unreadable kill; historical retry cannot clear fault; fresh start can | `restart_admission::restart_kill_switch_refuses_resume` | Real handler with durable fault and no guest records |
| Older WAL lacks authority; fresh start replaces it | `execution::managed::restart_tests::restart_missing_context_faults_until_fresh_authorized_start` | Actual older-format frame without the optional fields, WAL reopen, durable fault, original retry vs distinct start |
| Atomic context/receipt and durable fault; mismatched context refused | `restart_context_and_fault_commit_with_receipts_and_survive_reopen`, `restart_context_missing_or_not_bound_to_original_receipt_is_refused` | Persistence seam |
| Recovered executable bytes match verified archive | `restart_code_validation_compares_recovered_bytes_to_verified_archive` | Persistence seam |
| Unknown guest attempt and pending stop fence resume | `restart_unresolved_attempt_and_pending_stop_prevent_resume`, `failed_stop_is_fenced_recorded_and_replayed_as_the_original_receipt` | No guessed guest outcome; pending cancellation is retained |
| Historical retry does not reverse stop after reopen | `lifecycle_operation_retries_cannot_reverse_later_transitions_after_restart` | Real iroh handler/WAL |
| Newer staged artifact does not replace installed identity | `start_and_restart_revalidate_installed_artifact_not_newer_staged_ref` | Installed audit lookup, not latest accepted ref |

Required `rust` CI runs the following command once:

```sh
RUST_TEST_THREADS=1 scripts/ci/cargo-test --locked -p coglet-runtime --features native-engine --test device_lifecycle
```

This includes both task-required names. Default workspace tests cover the managed
persistence controls. Repeated stress is **not** a required-job retry:

```
python3 crates/coglet-runtime/tools/qualify-restart.py \
  --output /absolute/new/evidence-directory --count 20 --load-workers 2
```

The runner requires a clean committed tree, records the exact SHA, each raw test
log, sequential outcome and live load-worker observations, plus measured worker
CPU seconds. Qualification at `5d8ce7c56af2bcfb267a49cc45f415e272f529a8`:
20/20 consecutive passes; two workers consumed 675.540711706 and 677.083679826
CPU seconds over 702.693358918 elapsed seconds. Raw logs and summary are retained
in this card's evidence archive; this snapshot is not a claim about later heads.

## Honest applicability limits

- The managed device seam has synchronous start/stop + terminal receipts and no
  durable queued pause API or paused state. Thus ADR's
  `restart_pending_pause_prevents_guest_entry` is **not implemented or claimed**;
  there is no real queued pause to fabricate. Existing pending-stop,
  staged-boundary and unresolved-attempt fences are checked before resume.
  Adding pause requires its own implementation and actual queued-mutation proof.
- SurfaceListener intentionally refuses an existing socket pathname. The SIGKILL
  proof's supervisor reaps its exact child, then removes that child's stale
  socket before restarting it. Production supervisors must do the same within
  their owned private socket directory. This does not qualify unattended stale
  socket reclamation by cogletd.
- The proof uses a post-restart event, not an uninterrupted timer or physical
  actuator. It verifies new committed execution, not replay of an uncertain
  guest or redispatch of an already committed physical effect. Native readiness
  means eligible when input is ready, not continuous guest CPU consumption.
- This is Linux process/socket qualification. It does not add iOS/device or
  acoustic acceptance and does not claim all ABI or lifecycle operations.
