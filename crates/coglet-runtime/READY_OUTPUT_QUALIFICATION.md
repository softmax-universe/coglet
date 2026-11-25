# Ready-output C ABI qualification (ADR-0034)

Status: **in progress, not full acceptance**. Owner: `t_01a210fc6cef991b`.
The docs-first ADR landed through PR #170 before these runtime changes.

## Model and fault boundary

Mandatory process/generation/clock/tick preflight precedes every receive. Tick
may remove revoked/expired reads or disconnect an inactive peer. An eligible
front after that preflight is queried/copied without optional `pump_dispatch`.
This is zero optional-pump work ahead of a ready front, not a hard real-time
bound on lock acquisition, preflight durability, OS scheduling, send, or an
already-entered empty receive. Copying is bounded by the frozen 1 MiB frame cap.
An independent native off-switch must not borrow the exclusively owned handle.

The per-link hold at the optional receive-pump entry is a **source-bound defect
class counterexample**, not attribution of Face failed01/failed02. Their historical
ABI/SDK clocks do not exist. The hold has an eight-second cleanup watchdog;
release and worker join must precede snapshot, close and runtime stop.

## Requirement to named test matrix

| Requirement | Named proof | Current boundary |
|---|---|---|
| Ready health bypasses obstructed optional pump | `native_engine_cases::ready::ready_output_not_obstructed_by_pump` | Real C entry, real WAMR setup and committed WAL reopen; 250 ms ordering observation only. Actual Swift SDK >=3 s timeout/native-stop pair still required. |
| Query/short/exact reads preserve front, no optional pump | Same test | Checks required length, unchanged front and zero hold entries. |
| Health latency and committed native usage | `native_engine_cases::ready::ready_health_latency_bounded_under_pump_load` | Real due timer native workload, two committed WAMR steps and journal usage; ready query/copy skips the pending step, then empty recv executes it. 500 ms environment threshold, not concurrent already-entered guest preemption. |
| Bounded recent history and first anomalous call | `ffi::timing::tests::timing_ring_is_bounded_and_retains_first_failure` | 100 calls, exactly 16 retained, first slow success retained even after later fatal. This slot means first slow-or-fatal, not a separate first-fatal ledger. |
| Constant-work non-private kind labels | `ffi::timing::tests::output_kind_matches_real_encoded_frames_without_private_payloads` | Labels only internally generated closed body/type CBOR. Not a general CBOR recognizer, schema validator or authority decision. |
| Test C capture and independent hold-control custody | `native_engine_cases::ready::ready_output_not_obstructed_by_pump` | Size/query JSON copy only after joining worker; bounded ring, Health kind/depth and unknown adapter FIFO. |
| Exact-deadline/revoked read, inactivity, panic/generation and empty progress | Existing surface/read-expiry/restart suites plus targeted ready-front controls | Must run and map exact test names before acceptance. |
| Actual Swift SDK health timeout + independent native stop; no-obstruction/control | Pending crate-owned SDK harness | Must use unchanged `SurfaceSession.tick/receive`, not a duplicate timeout implementation. |
| macOS Swift SDK suite | Pending macOS execution | Linux C-pointer proof does not qualify Apple platform execution or iOS suspension. |

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

## Outstanding release gates

Actual SDK obstruction/control oracle; genuinely loaded guest qualification;
ready-front authority controls; current-head default/workspace/native tests,
clippy, fmt, primary ABI and surface tools; Mac Swift suite; independent exact-head
review, required exact-head CI and terminal MERGED implementation PR. Frozen
negotiation-role corpus and operational native tests must be reported separately.
No consumer acceptance, historical cause, physical latency or iOS claim yet.
