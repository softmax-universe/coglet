# Ready-output C ABI qualification (ADR-0034)

Implementation owner: `t_01a210fc6cef991b`, PR #191. Docs-first ADR landed
through PR #170 before implementation. Required exact-head CI and queue landing
remain release gates; local evidence below does not waive them.

## Model and fault boundary

Mandatory process/generation/clock/tick preflight precedes every receive. Tick
may remove revoked/expired reads or disconnect an inactive peer. An eligible
front after that preflight is queried/copied without optional `pump_dispatch`.
This is zero optional-pump work ahead of a ready front, not a hard real-time
bound on lock acquisition, preflight durability, OS scheduling, send, or an
already-entered empty receive. Copying is bounded by the frozen 1 MiB frame cap.
An independent native off-switch must not borrow the exclusively owned handle.

The per-link hold at optional receive-pump entry is a **source-bound defect
class counterexample**, not attribution of Face failed01/failed02. Their historical
ABI/SDK clocks do not exist. The hold has an eight-second cleanup watchdog;
release and worker join precede snapshot, native free and runtime stop.

## Requirement to named test matrix

| Requirement | Named proof | Executed evidence / boundary |
|---|---|---|
| Ready health bypasses obstructed optional pump | `native_engine_cases::ready::ready_output_not_obstructed_by_pump` | Real C entry, WAMR setup, committed WAL reopen; old ordering RED/fixed GREEN. Short Rust hold is an ordering proof; actual SDK timeout proof is separate below. |
| Query/short/exact reads preserve front, no optional pump | Same test | Required length, unchanged front, zero hold entries. |
| Health latency under pending native workload | `native_engine_cases::ready::ready_health_latency_bounded_under_pump_load` | Due timer, two committed WAMR steps/nonzero compute; ready query/copy skips pending step, then empty recv executes it. 500 ms environment threshold, not preemption of an entered guest. |
| Bounded history and first anomalous call | `ffi::timing::tests::timing_ring_is_bounded_and_retains_first_failure` | 100 calls, exactly 16 retained; first slow success survives later fatal. First slow-or-fatal, not separate first-fatal ledger. |
| Constant-work non-private kind labels | `ffi::timing::tests::output_kind_matches_real_encoded_frames_without_private_payloads` | Closed internally generated CBOR only; not a general parser or authority decision. |
| C capture and independent hold custody | `native_engine_cases::ready::ready_output_not_obstructed_by_pump` | Capture only after worker join; Health kind/depth, adapter FIFO unknown. |
| Exact/late read expiry, negative completion and reasoning fence | `execution::heard_deadline_tests::reply_at_or_after_published_deadline_completes_negatively_and_keeps_reasoning_fenced` | Native library suite passes; ADR-0029 unchanged. |
| Revoke/kill/disconnect authority | `execution::heard_deadline_tests::expired_heard_completion_is_fenced_by_revoke_kill_and_disconnect` | Native library suite passes. |
| Slow storage cannot renew authority/inactivity | `execution::heard_deadline_tests::slow_store_does_not_renew_expired_grants_or_inactivity` | Native library suite passes. |
| Session replacement purges old reads/results | `operational_surface::concurrent_session_epoch_purges_old_read_reply_and_completed_result` | Operational surface suite passes. |
| Three-second inactivity | `surface_health::idle_tick_emits_health_then_disconnects_at_three_seconds` | Exact surface deadline control passes. |
| Ready output cannot bypass panic, stop generation or inactivity | `ffi::tests::caught_panic_fences_all_links_and_stop_discards_poison` | Actual queued hello C-pointer controls, including 4s past-deadline inactivity, pass. |
| Daemon restart without new start; tamper refusal | `device_lifecycle::running_coglet_resumes_execution_after_daemon_restart`, `device_lifecycle::restart_revalidation_refuses_tampered_artifact` | Native lifecycle suite passes; ADR-0027 unchanged. |
| Actual Swift SDK health timeout/local stop and fixed/control | `native_engine_cases::ready_swift::swift_sdk_ready_health_obstruction_and_control` | Mac mini at `dd04d43`: fixed 1 pass/0 failures/0 ignored; control, receive-error and obstruct modes execute. Isolated old-order mutation removes only ready-front guard: control/error pass; obstruct enters pump, SDK >=3s timeout/local stop, zero effects, expected test exit 101. Original WAL reopened/nonzero WAMR compute. Local callback is not physical stop. |
| macOS Swift SDK package | `swift test` in `sdk/surface-swift` | Mac mini 33 pass/0 failures, exit 0. Not iOS suspension qualification. |

## Evidence and suite boundaries

At `dd04d43`, Linux full `native-engine,ffi-test-hooks` suite exited 0:
64 library tests, 38 native operational tests (zero ignored), 43 lifecycle tests,
and 25 operational surface tests pass. Whole-suite ignored registrations include
the subprocess helper `retry_crash_child` (invoked by ordinary crash tests) and
the separately selected simulator heard-history test; this is not a zero-ignore
claim for the entire package. Native FS subprocess actually executes.

`cargo test --workspace --locked` failed only at
`pinned_transform_cross_arch_replay`: the local worker lacks the required same-run
native architecture pair. An explicitly filtered rerun skipping that one test
passed; it is **partial local workspace coverage**, not full workspace acceptance.
Unfiltered required CI must provision and execute the pair. Default all-target
clippy found two feature-only helper functions compiled without their callers;
they are now gated by the same `ffi-test-hooks` feature, without suppressing warnings.

Frozen role-selected negotiation tools remain separate from operational native
proofs. `check-primary-abi.py` covers only the primary ABI subset;
`check-surface.py` covers two frozen runtime negotiation cases per transport and
pointer probes, not all operational behavior.

Durable raw evidence on the hub under
`~/.hermes/kanban/evidence/t_01a210fc6cef991b/`:
- `ready-original-red-green.tar.gz`, SHA256 `264ad8c73d203c3ce031229aafb3a49091455c312524062525d8f91dfe49b42d`;
- `ready-native-dd04-evidence.tar.gz`, SHA256 `671cff03373de05922036ad93a7d54b09ba1a565c2cac8bb99a72322dde93a49`;
- `mac-sdk-final-evidence.tar.gz`, SHA256 `42a225759606daff916a84c23cb17422423a5bad3f89f16e3b68259a41766b1f`.
The final Mac archive contains safe deferred-close fixed/old-order logs, exact
mutant patches and Swift suite output. Earlier unsafe host experiments are not
used as acceptance evidence.

## Instrumentation deployment boundary

All builds retain a fixed sixteen-call history plus first fatal-or-1s residence
record. Samples share a per-link monotonic epoch: entry, clock/tick, ready,
pump entry/exit, return, sequence/status, front byte length and output kind/depth.
No body bytes, argument values, identities, transcript or audio are retained.
The kind recognizer examines constant-size suffixes of internally generated,
validated closed messages; it is not used for protocol decisions.

`ffi-test-hooks` additionally exports `coglet_surface_test_timings` (bounded JSON
size/copy convention) and independently owned pump-hold controls. These symbols
are **absent from ordinary product builds**. Snapshot requires exclusive handle
custody and records completed/unwound calls, not a concurrently executing call.
The hold control alone may be inspected/released independently. Never free that
control while it has users. SDK uptime and native epoch are separate clock
domains; pair calibration samples, never subtract unrelated epochs. Adapter FIFO
depth stays null/unknown; runtime output depth is not an adapter FIFO measure.
SDK logical close remains immediate; the diagnostic host defers actual free until
release/join/capture and verifies exactly one eventual native close.

No historical cause, hard real-time, physical/acoustic latency or iOS claim is made.
