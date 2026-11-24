# Go kill-latched timing repair (t_01a20dcb7a8f60db)

## Evidence and five whys

Problem: required Go run [45400144208 attempt 1](https://github.com/softmax-universe/coglet/actions/runs/45400144208/attempts/1)
failed `GoEmitterTests.test_go_module_tests` -> `TestSurfaceConformance` ->
`surface-go-reference.surface/kill-latched`, step 7 `TimeoutError`.
`original-failure.log` preserves the actual failed log. Attempt 2 succeeded at the
same head `dce02beb1cd38349e7e3f0563ba6ab7973c4d949` (independently queried).

1. Why did step 7 fail? Its receive exceeded the unchanged case's 200-ms
   wall-clock watchdog (`conformance/surface/kill-latched.json`, final step;
   `conformance/harness.py:timeline`). The historical log contains no syscall or
   scheduling trace: the exact source of that particular delay is **undetermined**.
   A 450-ms owned scheduler pause reproduces the same post-kill timeout on the
   actual adapter with real durability; no responses are mocked.
2. Why can runner scheduling fail a functional fixture? Go had no
   `fixture_clock = true` registration and `cmd/conformance/main.go` used
   `time.Since(started)` for its surface clock and independent kill. Consequently
   `timeline` selected the case's wall budget instead of ADR-0021's bounded I/O
   watchdog. `check_fixture_clock.py` runs the unchanged kill-latched case on both
   clock modes: the controlled pause fails wall mode and passes logical mode.
3. Why was Go still exposed after ADR-0021? Its four Rust reference/mock adapters
   implemented the out-of-band control socket, but the Go adapter's independent
   `ServeFD` loop had no equivalent clock dispatch/flush acknowledgement hook.
   The registry opt-in was per implementation, not validated for Go.
4. Why wasn't this caught? `TestSurfaceConformance` ran the registered claim but
   did not assert Go's deterministic-clock opt-in or inject a runner scheduling
   gap. Existing Go SDK timing tests use an injected atomic clock (`newHost`),
   not the adapter's real wall-clock fallback. Neither tests the missing binding.

Root (happened): deterministic fixture time was implemented only in the Rust
adapters, leaving Go's functional case coupled to runner wall time.
Root (not caught): no Go binding/opt-in failure-injection regression; injected SDK
clock tests were incorrectly assumed to qualify the adapter's live clock.
Recurrence: ADR-0021 already records step-7 kill-latched failure in run
45341088504 and distinguishes infrastructure time from protocol time. This repair
extends that existing decision to Go; no frozen case, wire or schema changes.

## Countermeasures and proof

- Symptom containment: PR128 wires the Go required-check wrapper to the existing
  quarantine list. It skips only the exact named id; stability's full/only-held
  registry runs still execute it. PR129 explicitly removes the entry.
- Root: Go adapter implements ADR-0021 control advances and dispatch/output-flush
  acknowledgements. Registry opts in. Non-opted-in wall behavior remains.
- Detection: `TestFixtureClockAcknowledgesAfterOutputAndRejectsRetrograde` checks
  acknowledgements, equality and rollback; `TestSurfaceConformance` also executes
  `check_fixture_clock.py`, which asserts registration and compares the same
  450-ms scheduling stall in both modes. Its reported `step 1` is original step 7
  because it executes the last two steps after the first six have completed.
- Live-time preservation: `check_live_clock.py` launches actual non-opted-in
  adapter processes. With no clock-control socket or inbound stimulus, idle health
  must advance past 1 second, a positive elapsed deadline must reject as expired,
  and independent local kill must fire after 100 ms with stopped health and
  rejection. Bounds are runner watchdogs, not production latency claims.
- Pool CPU acceptance: `cpu-load-20.log` records 20/20 consecutive unchanged
  kill-latched passes with adapter, harness and two owned CPU burners pinned to
  the same allowed CPU. Burners exited cleanly. Reproduce from repo root:
  `go build -C sdk/surface-go -o ../../../go-surface ./cmd/conformance` then
  `python3 docs/reviews/go-kill-latched/repeat.py ../go-surface`.
- Local commands: `go test -race ./...` and `python3 ../../codegen/test_go.py`
  passed in `sdk/surface-go` (11 emitter tests).

The earlier reclaimed worker's claimed 30-pass run has no attached raw evidence;
it is **not** used for acceptance. The fresh 20-pass log is retained here instead.
CPU stress proves this controlled cohort, not all storage/scheduler conditions.
No evidence attributes the historical failure to a particular fsync syscall.

## Stability dispatch boundary

Runs 45407854347 and 45409917358 failed before reusable conformance jobs were
created. The GitHub web summary shows `evaluate reusable workflow inputs:
.github/workflows/stability.yml (Line: 32, Col: 26): Unexpected value '20'`.
The jobs API reports only the two successful top-level jobs, hiding this cause.
The dispatch's numeric repeat value crosses a strict workflow-call number input;
explicit `fromJSON(format('{0}', ...))` coercion preserves a numeric value for both
string and number callers. Live dispatch on the final branch must verify this;
passing the two top-level jobs alone is not stability success.

Independent exact-head review, green required CI, green stability, queue landing,
and postmerge checks remain separate gates; local proof does not waive any gate.
