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
