# Factory Phase 4 — research and evaluation

Private application data in Factory Wasm, not a database migration, protocol
extension, scheduler or host evaluation service. Phase 3 goal admission and
cancellation remain unchanged; goal-owned model dispatch is still forbidden.

## Lifecycle

1. `INVESTIGATE` on a failed task creates a diagnostic task and hypothesis. When
   the failure belongs to an experiment, a durable `informs` edge connects the
   failed experiment to the new hypothesis. The existing `DISPATCH` operation
   handles the diagnostic; failures and replies retain existing event receipts,
   stale-context validation and worker/budget gates.
2. On a hypothesis review, private `EXPERIMENT {experiment_id,task_id,
   hypothesis_id,plan}` creates one experiment, one ordinary task and a `tests`
   edge. Only the reviewed open hypothesis is eligible. It may be followed by
   `DISPATCH` for that task, using the existing durable outbox. No other compound
   research mutation is admitted. The dispatch payload carries `task_id`,
   `experiment_id` and JSON-text `research_plan`, so the worker does not need
   access to the Factory's private database.
3. The normal adapter's durable result note carries the closed `Report` below.
   The executive fences task, attempt, retained lease and terminal state *before*
   ingesting it. The experiment `produces` an immutable evidence object, recording
   the event's authenticated source/sequence, correlation, attempt, lease, exact
   note hash, experiment/task/hypothesis bindings, plan and validated report.
   Failed, cancelled, missing, malformed or mismatched reports are retained as
   `inconclusive`, not improvements. A failed task enters the existing async
   investigation path; successful research enters async evaluation (or a bounded
   durable deliberation row when another request is pending).
4. `LEARN {knowledge_id}` is a single-operation proposal on the evaluated
   experiment. It rereads the stored evidence and its experiment-held SHA-256,
   validates every binding and recomputes the comparison. A fresh knowledge record
   copies the complete evidence and digest. Evidence `informs` knowledge;
   improvement `supports` the hypothesis, regression `contradicts`, and unchanged
   results only `inform`. Inconclusive evidence cannot produce knowledge.
   The experiment becomes `concluded`. Plain `EVALUATE` prose has no authority to
   create evidence, knowledge, a comparison verdict or a promotion.

No synchronous provider/network calls occur in handle. Storage/capability errors
in research validation propagate separately from semantic proposal rejection.
Every operation remains in the runtime-owned DB/files/journal transaction. No
paging, unbounded scans, new trace store, paid model calls or live Board fixture
writes are introduced.

## Pinned comparison and trust boundary

`Plan` is a closed JSON object:

```
{baseline,candidate,inputs,responses,config,evaluator,budget}
```

The six string fields are bare 64-lowercase-hex content hashes, referencing
existing immutable code, inputs, recorded model responses, configuration and
versioned evaluator artifacts. Baseline and candidate code must differ. `budget`
is a positive **per-run evaluation-unit** ceiling, distinct from the existing
Factory dispatch-admission count and gateway monetary authorization.

`Report` is closed JSON in the worker result `note`:

```
{experiment_id,task_id,baseline:Run,candidate:Run}
Run = {run_id,code,plan,trace,artifact,cost,score}
```

Run IDs are distinct 32-lowercase-hex strings. Each run must repeat the exact
pinned plan; its code must match the appropriate variant. `trace` and `artifact`
are immutable 64-lowercase-hex references. Each cost must fit the same plan budget.
The pinned evaluator defines an integer higher-is-better score; null/missing
scores are inconclusive, never zero. Comparisons use ordering, not subtraction
(which could overflow), and classify improvement/regression/unchanged/inconclusive.

These references establish identity and provenance binding, **not independent
proof of worker honesty, blob availability or evaluator correctness**. The
embedding host/receiver authenticates the worker and grants artifact access;
the Factory does not download or verify blob bytes in handle. A trusted worker
must run the pinned evaluator/recordings and publish its immutable artifacts.
Tests use deliberately synthetic fixture hashes and recorded/mock responses;
they are not measured performance improvements or production deployments.

## Acceptance mapping

| Acceptance name | Concrete test |
|---|---|
| `failed_experiment_triggers_investigation` | Native `tests/research/acceptance.rs`; real WAMR `crates/coglet-runtime/tests/factory_research/mod.rs` |
| `evidence_provenance_is_preserved_and_validated` | Native `tests/research/acceptance.rs`; same real-WAMR module |
| `research_replay_uses_recorded_responses` | Same real-WAMR module, `coglet-llm` + `coglet-mock::llm`, immutable inbox, reopen and offline `replay_database` after removing both SQLite databases |
| `candidate_baseline_comparison_is_reproducible` | Native `tests/research/mod.rs`; same real-WAMR module, repeated fixtures plus regression/tie/inconclusive cases |

Annotations are type-gated: generic EVALUATE/REPLAN/PRIORITIZE/DEFER cannot
mutate experiment, evidence or knowledge payloads. Multi-row deliberation
selection uses a strictly read-only page and a bounded transactional guest-file
identity/revision, then revalidates before the next step mutates anything; no
post-write existence query may mint a cursor. The real-WAMR
`factory_research::backlog::two_research_completions_drain_via_revision_fenced_read_only_selection`
test queues two completions behind another provider request and drains both
through emitted timers across reopen.

Additional native regressions in `tests/research/regressions.rs` reject mismatched
input/config/recordings/evaluator/budget/code/task/experiment/run identities,
malformed hashes, cost overruns and unknown fields; `backlog.rs` consumes an
emitted timer to recover a pending evaluation. Existing Phase 3 goal budget,
priority, deferral and cancellation suites are retained.

Run all actual guest acceptance with:

```sh
cargo test --locked -j2 -p coglet-runtime --features native-engine \
  --test factory_database factory_research -- --test-threads=1
```

The owned `coglets/factory/check.sh` discovers these modules in the existing
required CI job. Nonzero real-WAMR runs, the full owned check, independent
exact-head review, required CI and main merge-queue landing are final gates;
this document does not assert those gates have completed.
