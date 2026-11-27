# Native simulator reset investigation

## Evidence boundary

Historical required-main Rust run45766928744 at c4365edea9585542d7e05b33ccf50208252102fb failed cartpole with ECONNRESET104 (30other tests passed). Matching merge-queue run45759777267 and original local script31passed. Historical daemon stderr was not retained: its exact cause remains UNKNOWN. The local counterexample below is a demonstrated mechanism, not retrospective proof of that run.

## Observed counterexamples

- Original exact-source script:31passed/0skips224.97s; guest-policy3passed (recovered run1 raw evidence archive SHA256296d8e65e0ba6d906193e826baaa58b9ffb313ed77aea015b1b7130e20ba742c).
- OneCPU affinity inherited by Python/native daemon plus2owned busy children: reset104 after8actions29.63s; retained stderr `surface connection: surface inactivity timeout`. Strace counterpart resetafter9actions36.80s: accepted action.outcome at1795681786.930, about10MB WAL writes, no socket read before inactivity error1795681790.036 while peer bytes remained queued. All ownedloadchildren reaped.
- Restored uninstrumented baseline on current worker:29passed/2failed (native-current.log); cartpole failedafter16actions, daemon `surface connection: fatal surface message`. This message alone does not establish the same timeout mechanism.
- Local-only duration instrumentation on reproduced loaded daemon: late engine.execute21.6ms, Runtime::reconstruct1599.7ms, recover_attempt2167.3ms, native run2744.2ms; subsequent send observes inactivity clocks33818777-30751621 (>3000000us). Saved local-phase-probe.patch is diagnostic-only, removed from production tree.

## Five whys — demonstrated local inactivity case

Problem: original native cartpole test resets the Unix socket under bounded oneCPU contention.
1. Why reset? Daemon closes the connection for surface inactivity. Evidence: loaded daemon stderr and syscall timestamps above.
2. Why inactivity despite peer bytes? Synchronous native completion consumes the service interval before the pump reads queued peer messages. Evidence: action.outcome receive followed by WAL work/no recv before the error; phase timings and observed inactivity delta.
3. Why so much host work? Every live detached completion calls recover_attempt, which rebuilds host state by replaying the entire committed segment plus the just-returned observation. Evidence: detached.rs/recovery.rs old call path and measured reconstruct1599.7ms versus actual engine21.6ms.
4. Why replay a validated prefix? Live native return reused general crash/external-recovery reconciliation. That route must reconstruct untrusted/reopened observations, but an original in-process coordinator already holds a validated prefix and exact durable attempt fence. Evidence: original recover_attempt/reconcile_attempt and original detached reservation/fence validation.
Root(happened): cumulative full-prefix host-transition replay on each live native step creates history-dependent work between legal protocol messages. This fixes that work class, not a universal3s/wait-free guarantee: WAL serialization/tree validation/engine work remain.
Root(not caught): coverage checked functional live/replay identity, not replay-work scaling; healthy/local and merge-group runs can pass. No failure artifact retained stderr/precleanup process status, so historical main diagnosis stopped at reset104.
Recurrence: searched and coordinated existing native code-pin owner PR199 and treehash owner PR192 (t_01a2084f99c437b8); neither owns this full-prefix reconciliation path. Historical run is only a similar symptom, not a proven recurrence of this cause.

## Countermeasures and proof

- Symptom/root: only original in-process detached Surface01 completion applies the returned observation to a private clone of validated live prefix, after original exact attempt/fence/router-prefix checks. ObservedTransition rechecks hostcalls/deltas and full resulting journal equality; production guest is never reexecuted. Full crash/external/database recovery remains full reconstruction. Publication retains the same WAL commit, dispatch reconciliation and runtime validation path. No protocol/deadline/grant/inactivity change.
- Scaling regression: live_detached_replays_only_new_observed_transition genuinely RED before repair(left3/right1) then GREEN(one observation regardless of historical prefix2or8), logs regression-red2.log/regression-green2.log.
- Differential/negative coverage: live_replay_differential_tests compares incremental versus original full reconciliation over committed/aborted/files/hostcall prefixes, nonempty boundary, operational source/time highwaters, queued-event interleaving, mutated hashes/observations and missing/consumed fences. No production fault switch.
- Detection: three actual subprocess diagnostic tests, including genuine Unix ECONNRESET104, establish bounded8192byte stderr, precleanup vs killed/reaped status, elapsed call timing and original exception preservation. Diagnostics never mask failure or change retry/deadline behavior.
- Original required simulator selection remains, adds diagnostic tests: actual repaired check-native.sh34passed/0skips (original31+3diagnostic), guest-policy3passed; initial runtime native library84passed. Final exact-head full suites/review, non-required20passCPU-load stability, required CI, queue and containing-main are still pending at this draft.

## Non-changes

No quarantine or skip; no blind required-job reruns; no timeout enlargement or drain-before-expiry; no code-pin/treehash optimization duplication; no historical causal certainty; no product/H-transform cutover or evidence waiver.
