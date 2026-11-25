# Simulator heard-history stability qualification

Owner: `t_01a20ffa7a32ef37`. Named acceptance test:
`sim_heard_history_journals_heard_prefix_and_replays_offline` in
`tests/sim_duplex_heard.rs`.

## Failure and root-cause boundary

Required rust run [45511158700](https://github.com/softmax-universe/coglet/actions/runs/45511158700)
failed at `runtime closed the session`; attempt 2 passed on the same source.
Quarantine landed in PR #155. A local loaded reproduction with temporary runtime
instrumentation recorded heard-read expiry (deadline 2982665, tick 3056045), then
session closure. The original CI failure was not instrumented, so its precise
branch cannot independently be established from that message alone.

Runtime policy is owned by `t_01a20fd6d43dfc22`: PR #156, merged as
`1c5642eff5866ac0e3b76482190cda83e3bc0ed7`, replaces early expiry disconnect with
negative native completion while preserving the durable heard barrier. That alone
does not restore this fixture's snapshots: the original wall-clock integration on
that merged runtime failed loaded iteration 3 (2 passes), with explicit expiry
at tick/deadline 3096934/3095579 and subsequent negative read reply.

The functional fixture coupled both runtime deadlines and Python SDK/media timers
to runner scheduling and durable I/O. Its drain-before-send loop withheld newly
queued reads; immediate read flushing alone also failed under load. A quiescence
`try_recv` could additionally consume and discard an inbound frame.

## Correction

Use the existing ADR-0024 scheduled Python peer, retaining the real Python SDK,
codec, ledger, media callbacks and runtime WAL. Both sides use logical time.
Per-dispatch acknowledgements and cumulative framed-stdout byte watermarks ensure
all output is processed before the next callback. Wall time is only an I/O watchdog.
Full heard snapshots, action correlation/outcomes, WAL reopen and independently
rerun offline replay assertions remain intact. This is functional integration
qualification, not physical-device latency or acoustic qualification. Production
clocks, timeouts, authorization and negative-expiry controls are unchanged.

## Measured loaded qualification

Test source commit: `b434e88fcc18fda0c8bd4b91fa9907aead0b7114`.
Command per iteration:

```sh
cargo test --locked -p coglet-runtime --test sim_duplex_heard -- \
  --ignored --exact sim_heard_history_journals_heard_prefix_and_replays_offline \
  --nocapture
```

All **50/50 sequential iterations passed**, each executing exactly one named test,
with three continuously live CPU burners sharing CPU 0 with the driver and its
children. Every iteration checked positive `/proc` CPU-time deltas for all three
burners and their liveness. Wall duration: 585.317438 seconds; burner process CPU
seconds: 149.346543, 149.366177, 149.356141. The runner exits nonzero on any test or
load-evidence failure. Raw per-iteration logs, indices, CPU deltas, summary and the
runner are retained in the owning card's evidence archive.

Non-required hosted stability run
[45534916429](https://github.com/softmax-universe/coglet/actions/runs/45534916429)
uses that same source with the quarantine entry retained and requests 50 repeats;
its `rust-quarantined` job passed. The aggregate workflow also repeats the full
conformance registry and must be checked separately before landing. Quarantine
removal changes no test source; final-head required CI and branch stability remain
landing gates. Stress loops are not added to any required job.
