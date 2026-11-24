# First-action conformance durability barrier investigation

## Observed failure and cause boundary

Runs 45333592246 and 45333746917 failed `action-accept-outcome` step 5;
45333593932 failed `dedup-terminal` step 5. Step 5 is the **first accept** in
both scripts, not terminal retry. The scripts share the initial action sequence.
The expected accept and outcome are required by surface-protocol.md; the harness
is not waiting on an optional event.

A syscall trace of the real release adapters shows `Admitted`, `Terminal`, then
`Seq` each written and fdatasynced before the adapter returns its response batch
and writes accept + outcome to stdout. Thus the accept's 200ms budget covers
THREE durable barriers. PR87 fixed initialization charging but did not change
this path. Normal local traces measured each barrier around 3ms; that is not a
worst-case bound. The historical runners have no syscall trace, so their specific
storage/scheduler cause remains unproven.

## Change and safety

Commit the existing Terminal and, when needed, Seq reservation frames in one
batch under one fdatasync. Admission is still separately durable before backend
execution. Terminal and event sequence reservation remain durable before wire
acknowledgement. No spec, wire format, case, or timeout changes. Replay already
supports independently framed records and a torn trailing frame. A crash before
the batch barrier can expose a prefix, but no event was acknowledged; the stored
terminal is available for reconciliation and no physical effect is replayed.
Write/sync failures poison the store; in-memory terminal/sequence state is not
published on failure. Sequence assignment still occurs on delivery; reservation
is not consumption. Full/partial reservation blocks and restarts are tested.

## Reproduction and acceptance logs

Linux, release binaries, strace. Two owned busy-loop processes, harness, and
adapters share ONE allowed CPU via inherited affinity. Processes are terminated
and reaped in finally. Each action-path fdatasync (ordinal 3 onward, NOT a single
removed ordinal) receives the identical delay in old/new runs.

```
cargo build --locked --release -p coglet-surface-rust -p coglet-mock --bins
# before binary copies were taken before changing Store
python3 conformance/repeat_action.py --iterations 3 --sync-delay-ms 70 \
  --binary-dir target/step5-before --expect-timeouts --output target/step5-before-70
python3 conformance/repeat_action.py --iterations 50 --sync-delay-ms 70 \
  --output target/step5-after-70
python3 conformance/repeat_action.py --iterations 50 --sync-delay-ms 0 \
  --output target/step5-cpu-only
cargo test --locked -p coglet-surface-rust -p coglet-mock
cargo clippy --locked -p coglet-surface-rust -p coglet-mock --all-targets -- -D warnings
```

The adjacent JSONL files contain every result and the measured CPU/wall-time
summary, not a simulated output:

- `before-70ms.jsonl`: 24/24 first-accept step-5 timeouts, zero skips.
- `after-70ms.jsonl`: 400/400 pass: 50 consecutive loops of BOTH named cases
  across reference/mock × stdio/in-process, zero skips.
- `cpu-only.jsonl`: another 400/400 pass under the same CPU contention without
  storage-delay injection, zero skips.
- `residual-75ms.json`: earlier, stronger 75ms experiment yielded 399/400 passes
  and one step-5 timeout. Preserved rather than silently dropping that result.
  This change reduces exposure, it cannot establish a scheduler-independent
  200ms response bound.

Regression `store::tests::terminal_reserves_delivery_sequence_before_returning`
was observed RED before implementation (`terminal commit must include delivery
reservation`) and GREEN after. Additional tests exercise failed writes, torn
sequence tails, reservation rollover and restart monotonicity. Existing SDK/mock
suites remain green. CI's existing twenty-full-run step remains required evidence;
local focused repetition is not a substitute.

## Additional operator report and remaining boundary

PR94 run 45335407065 at 0af078ab6b76d74ba4b95bda7e68f671198395cf
also reports read-config step 2 (capabilities) timeout after readiness. That step
includes the grant-epoch durability barrier before the capabilities/grants batch
is returned. This optimization does NOT remove that required barrier, nor claim
to fix arbitrary single-sync stalls exceeding 200ms. No deadline change is hidden
in this PR. New CI failures must be investigated, not waived or retried into green.

Root (happened): redundant independently synced sequence reservation inflates the
first-accept response path's exposure to storage/scheduling stalls. Root (not
caught): startup-only injection in PR87 never perturbed action-path syncs.
Countermeasures: batch terminal + sequence; explicit restart/failure regressions;
CPU-loaded old/new uniform action-sync-delay control. Historical host attribution
and general durable-response latency policy remain distinct evidence questions.

## Residual timing-family repair (ADR-0021)

The queue did expose that residual: readiness run45341088504 failed read-config
step2 and kill-latched step7. Baseline main run45340588562 also failed negotiation
and deadline-exclusive admission after readiness. We read both failed logs; no
failed job was rerun into green. PR100 was explicitly dequeued before revision.

ADR-0021 separates adapter fixture time from the runner watchdog with an explicit
private test-control socket. Its initial proposal was submitted as ADR-0020 in
913eb39 before implementation; concurrent vocabulary PR105 reserved0020, so the
proposal was renumbered0021. Existing package/path/wire identifiers are unchanged.
The implementation drives actual receive/tick and persistence, not canned replies.
The prior durability batching is retained. Production timing requirements remain
unchanged; deterministic functional tests do not claim live latency qualification.

Executed commands and raw evidence:

```
python3 -m unittest discover -s conformance/tests -p test_fixture_clock.py -v
python3 -m unittest discover -s conformance/tests -p test_readiness.py -v
cargo test --locked -p coglet-surface-rust -p coglet-mock
cargo clippy --locked -p coglet-surface-rust -p coglet-mock --all-targets -- -D warnings
python3 conformance/repeat_surface_clock.py --iterations 50 --output target/clock-final-cpu
python3 conformance/repeat_surface_clock.py --iterations 1 --sync-delay-ms 250 --without-fixture-clock --expect-timeouts --output target/clock-red
python3 conformance/repeat_surface_clock.py --iterations 1 --sync-delay-ms 250 --output target/clock-final-delayed
python3 conformance/repeat_surface.py --iterations 50 --slow-startup --without-readiness --output target/clock-startup-red
```

- `clock-final-cpu.jsonl`:50 complete loops of all four adapter variants under
  two owned busy processes sharing one CPU:2200 results,1800 executed passes,
  400 unsupported-role skips,zero failures. Each of action-accept-outcome,
  dedup-terminal,read-config,kill-latched executed200 times without failure.
- `clock-red.jsonl`:same current binaries, old wall-clock binding, identical250ms
  delay on EVERY fdatasync from ordinal2 (negotiation) onward:36/36 executed
  cases time out,8 expected skips. This is a deterministic mechanism control,
  not an attribution of historical runner stalls.
- `clock-final-delayed.jsonl`:identical perturbation with explicit fixture-clock
  binding:36/36 executed cases pass,8 expected skips. One full loop only; the
  50-loop acceptance above is CPU-only, not50 loops with250ms storage delays.
- `clock-startup-red.jsonl`:clock binding and readiness both disabled for the
  old exec-only startup counterfactual;450/450 executed cases fail at step0,
  100 expected skips. The clock connection is itself a startup barrier, so it
  must be disabled to preserve this counterfactual.
-10 new harness tests and7 existing readiness tests pass. New controls include
  slow real child I/O with unchanged logical time, missing/bad control ack,
  per-step and whole-case watchdogs, early child exit, inherited env isolation,
  unexpected frame and EOF failures. Rust tests cover exact ack time and
  rollback refusal. SDK/mock suites and clippy pass; logs adjacent.

The original20-run CI proof and independent approval cover d74a69f ONLY. New
exact-head20-run CI, readiness CI, independent review and queue/postmerge proofs
are still required for this revision; these local logs do not satisfy those gates.
