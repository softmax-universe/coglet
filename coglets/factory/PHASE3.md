# Factory Phase 3 — goal generation and prioritization

Phase 2 is the base. This is not a deployment.

## Implemented ingestion slice

An authenticated immutable inbox message with event kind `factory.goal` carries
this private, closed JSON payload:

```json
{"goal_id":"44444444444444444444444444444444","description":"example","priority":9,"budget":2,"depends_on":[]}
```

The embedding receiver supplies authentication through the existing `hub.inbox`
boundary; the guest validates the inbox schema, exact line/index/message ID,
sender/event-source and correlation/wake binding. Direct `factory.goal` events
are not accepted as an ingestion path. The deployment must separately provision
`factory-goal-authority`, an active `policy` object with closed JSON
`{"user_sources":["<authenticated coglet-id>"],"worker_sources":[]}`. Either list
may authorize a source; payload fields cannot claim a role. Missing authority
fails closed. The total allowlist is bounded at 64 entries.

Goals require a fresh 32-lowercase-hex identity, a nonempty bounded description,
priority within the existing trusted `factory-policy`, positive admission-unit
budget and at most two distinct existing goal/task dependencies. Creation stores
the goal, an immutable semantic origin, and `depends_on` relations atomically.
The origin allows identical creation under a *new* event identity without making
a second logical goal, even after later goal-state edits. Conflicting reuse is
rejected. Frozen `(source, seq)` receipts separately deduplicate transport replay.
No dispatch, LLM call, work-queue mutation, live Board call or migration occurs in
this slice. The only catalog addition is approved parameterized relation insert.

## Tests exercised for this slice

- `coglets/factory/tests/goals/mod.rs` (included by executive.rs):
  `authenticated_goal_ingestion_has_durable_identity_and_relations`,
  `unauthorized_and_unbound_goal_ingestion_is_refused`,
  `invalid_goal_and_failed_step_leave_no_partial_relations`.
- `crates/coglet-runtime/tests/factory_goals/mod.rs` (included by
  factory_database.rs):
  `factory_goals::factory_wasm_goal_ingestion_reopen_and_offline_replay` executes
  the actual WASI Factory on WAMR, reopens the runtime and SQLite around inbox
  delivery, verifies deduplication through a second genuine inbox boundary, and
  replays both segments after deleting the live database. Receiver authentication
  is an embedding fixture, not a new authentication implementation.

Native workspace run: 23 executive tests (including these three), 7 repository
and 22 host-dispatch tests passed. Focused WAMR run: 1 passed (nonzero), using:

```sh
cargo test --locked -j2 -p coglet-runtime --features native-engine \
  --test factory_database \
  factory_goals::factory_wasm_goal_ingestion_reopen_and_offline_replay \
  -- --exact --test-threads=1
```

The owned `coglets/factory/check.sh` remains the final full check; passing these
focused tests does not substitute for it.

## Review, admission, deferral and cancellation

- **Commands** (authenticated `runtime.inbox`, same goal-authority allowlist):
  `factory.goal` (create), `factory.goal_cancel {goal_id}`, `factory.goal_defer
  {goal_id, until}`. Cancellation is terminal and idempotent; deferral never
  touches a cancelled/active/terminal goal. Model `CREATE_GOAL` now carries
  `priority` and `budget` (defaults 0/1, bounded by policy) plus a
  `derived_from` relation; `PRIORITIZE`/`DEFER` on a goal subject use the same
  fields. Decisions for a cancelled/completed/exhausted subject are rejected and
  stale replies for them are never replanned.
- **Periodic bounded review** (`factory.goal_review` or the `runtime.timer` it
  arms): scan steps are strictly read-only pages of 4 goals (`open`, then due
  `deferred`) with the continuation, a constant-size best candidate and the last
  processed `(source, seq)` in the transactional guest file `/data/goal-scan.json`.
  A cursor invalidated by any write restarts the scan deliberately. The final
  step re-reads fresh goal/dependency/policy state and, only if the same
  candidate is still eligible, admits it: goal budget -1, task object, one
  `factory.dispatch` outbox intent (existing attempt-aware dispatcher), global
  budget and worker -1, audit event, receipt. No LLM, network or backlog cap.
- **Order**: higher priority first, then lexicographically smaller goal id.
  Eligible = open or due-deferred, per-goal budget > 0, dependencies
  (`context_ids`, <= 2) completed, global budget and a free worker.
- **Goal-task results**: completion completes the goal; failure/cancel returns
  it to `open` (budget left) or `exhausted`, never to the model, so retries obey
  the goal's own budget. A cancelled goal stays cancelled.
- **Cancellation of a dispatched attempt** is intentionally not a guest command:
  the `goal.cancelled` audit event names the active task and the host's
  lease-fenced `cancel_intent` path produces the `task.cancelled` result, which
  the executive already fences by attempt and lease. `HttpKanban` cancel remains
  unsupported and fails closed.

## Acceptance mapping

| Name | Native (`coglets/factory/tests/goals/acceptance.rs`) | Real WAMR (`crates/coglet-runtime/tests/factory_goals/acceptance.rs`) |
| --- | --- | --- |
| `goal_budget_and_priority_enforced` | yes | yes (7 goals, multi-page cursor, reopen) |
| `goal_replay_does_not_duplicate` | yes | yes (reopen + offline `replay_database`) |
| `goal_deferral_and_cancellation` | yes (real command events, stale model reply) | yes (seeded deferred/cancelled state, logical time advance) |

The WAMR cancel/defer *commands* are covered natively; the WAMR run proves that
cancelled and not-yet-due goals never dispatch and that a due deferral resumes.
Run the focused WAMR tests with `cargo test --locked -j4 -p coglet-runtime
--features native-engine --test factory_database factory_goals -- --test-threads=1`.
The owned `coglets/factory/check.sh` remains the final full check.
