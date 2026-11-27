# Evaluator heartbeat conformance mitigation

This is **noncausal mitigation**, not proof of the historical main ConnectionResetError104 or the separate local run11 failure. Those causes remain unknown and the repair owner's causal/qualification/merge gates remain open.

## Evidence and why chain

Problem: a connected, negotiated peer withholding application frames received capabilities/grants/event but no scheduled health from the original evaluator (actual `test_evaluator_health_during_silence` RED on parent5f55971; GREEN after the change).

1. Why no health? evaluate() waited in exact()/blocking recv before surface.tick()/drain().
2. Why did silence block scheduling? the evaluation loop coupled transport receipt to SDK timer service; sendall could also block that same owner.
3. Why not caught? native episode tests cover responsive execution/live replay but had no silent-connected-peer heartbeat oracle. SDK transport.serve already independently pumps timers, but evaluation does not use that complete server lifecycle.

Root (happened): evaluator scheduling, not a change to SDK policy or a demonstrated historical reset cause.
Root (not caught): missing evaluator silence/fragmentation service coverage.
Recurrence: follows retained native-reset investigation; earlier runtime performance repairs and passing captures did not establish the historical cause.

## Countermeasure and invariants

The private single-owner _EvaluatorLink uses SDK Frames/Output and nonblocking select. It services SDK tick/revocation/drain during fragmented reads and writes. No thread, separate writer, retries, protocol/spec changes, timeout enlargement or skips. SDK3s valid-inbound expiry remains independent of local sends/partial bytes. Frames remain bounded and app frames FIFO (existing kill/stopped-health priorities retained). The5s receive no-byte-progress budget and per-head5s total send budget remain; absolute90s episode remains anchored after hello transmission. Missing terminal response cannot pass solely because world.done. Final correlated response flushes queued outputs before closing; fatal paths discard without sending and retain original diagnostics/reap ownership.

Proof: `sim/tests/test_evaluator_heartbeat.py` real credential-checked silent subprocess regression (genuine RED/GREEN); `test_evaluator_link.py` deterministic socketpair/SDK and explicitly labeled minimal-surface deadline characterization; `test_evaluator_terminal.py` explicitly controlled evaluate/SDK/socketpair/real-child cleanup tests, deliberately refusing proof generation (not native replay evidence). Only the original native episode tests and independent native proof establish actual live/replay equality. Run the additional files with `uv run --project sim --locked pytest sim/tests/test_evaluator_heartbeat.py sim/tests/test_evaluator_link.py sim/tests/test_evaluator_terminal.py -v` (also discovered by ordinary simulator Python CI).

Scheduling remains cooperative: select wakeups cannot preempt arbitrary SDK storage stalls, native blocking elsewhere or process descheduling. Conformance controls do not exonerate historical counterexamples or authorize a new stress capture.
