# Factory cognitive architecture (v1 proposal)

Status: Design proposal; requires ADR review before protocol/spec changes.
Date: 2026-11-22

## Summary

The Factory is a self-modifiable Wasm coglet with a persistent, private cognitive state and a single event-driven executive. It manages goals, tasks, hypotheses, experiments, knowledge and artifacts; delegates execution to Hermes initially; evaluates outcomes; and proposes candidate code changes through the hub's existing promotion gates. Intelligence and strategy live in Factory Wasm; database, LLM, worker execution, and artifact storage are host-provided capabilities. The kernel alone enforces grants, promotion, probation, and rollback.

This document describes the intended v1 architecture, not an assertion that the interfaces already exist. It supplements the runtime design and does not supersede existing ABI/hub specs without an accepted ADR.

## Boundaries

- **Factory Wasm (evolvable):** event handling, context retrieval, goal management, prioritization, planning, prompts, decision validation, experiment selection, repository logic, and outbox intent generation.
- **Host capabilities:** parameterized database operations and short transactions, asynchronous LLM requests, Hermes adapter, artifact access, code/build/test/evaluation services. No database engine or credentials inside Wasm.
- **Hub/kernel (trusted):** identity, capability grants, event routing, budgets, replay recording, code verification, deployment and rollback. Inter-coglet messages go through the hub, never through shared database tables.
- **Private by default:** each Factory owns its own logical database; no other coglet can read goals, hypotheses, or knowledge without explicit grants or published artifacts/APIs.

## Storage design

Start with native SQLite behind a host-provided database capability; later move to managed PostgreSQL (e.g. RDS) when hosting/concurrency warrants it. The Factory's logical repository layer must not depend on SQLite-specific SQL. Do not build PostgreSQL CI initially; add cross-backend behavioral tests when queries/schema stabilize.

Initial relational layout:

| Table | Key fields | Purpose |
|---|---|---|
| `objects` | `id`, `type`, `status`, `data_json`, `version`, `created_at`, `updated_at` | Extensible typed cognitive records |
| `relations` | `id`, `source_id`, `target_id`, `type`, `data_json` | Typed edges between objects |
| `events` | `id`, `object_id` nullable, `event_type`, `payload_json`, `created_at` | Append-only meaningful cognitive decisions/changes |
| `work_queue` | `task_id`, `state`, `priority`, `lease_owner`, `lease_until`, `attempt`, `version` | Operational scheduling and retries |
| `outbox` | `intent_id`, `idempotency_key`, `kind`, `payload_json`, `state`, `attempt` | Durable external action intents |
| `schema_migrations` | `version`, `applied_at` | Database evolution |

Types of objects initially include `goal`, `task`, `hypothesis`, `experiment`, `knowledge`, and `artifact`. The ontology may expand without SQL migrations. Typical relations: `decomposes_into`, `depends_on`, `tests`, `supports`, `contradicts`, `produces`, and `informs`. Large traces, code snapshots, datasets, and reports are immutable blobs outside SQL, referenced by content hash and URI. Artifact access remains permissioned.

Use application-generated UUID/ULID IDs, foreign keys and indexes, explicit versions for optimistic concurrency, portable SQL joins/transactions, and opaque JSON text for non-indexed attributes. Avoid `INSERT OR REPLACE`, engine-specific JSON queries, and reliance on SQLite's single-writer semantics. Promote hot fields to indexed columns when necessary. Keep scheduling and outbox integrity in dedicated tables rather than generic JSON.

The Factory repository API provides operations such as `get_object`, `query_objects`, `get_relations`, `append_event`, and transactional updates. The database capability itself remains generic and thin: parameterized statements, bounded results, and short logical transactions. Precise ABI, paging, limits, authorization, and transaction lifecycle are deferred to a capability spec/ADR.

## Event-driven executive

One executive serializes agenda mutations in v1; many workers may execute concurrently. Each invocation is bounded and never blocks on a worker or model.

1. Receive event batch or timer.
2. Load a bounded, relevant context from the private store.
3. Apply routine deterministic state transitions without an LLM.
4. When judgment is required, submit an asynchronous LLM request and return. Persist request ID, context/state version and purpose.
5. On the response event, validate the proposed structured operations and their preconditions against current state; discard/replan stale decisions.
6. Atomically commit cognitive updates, important events, and external action intents to the outbox.
7. Dispatch intents asynchronously with stable idempotency keys; record outcomes on later events.
8. Sleep until the next event/timer.

Supported logical decisions initially: `CREATE_GOAL`, `DECOMPOSE`, `PRIORITIZE`, `DISPATCH`, `INVESTIGATE`, `EVALUATE`, `REPLAN`, and `DEFER`. These are Factory-owned schemas, **not** kernel commands. A context builder selects only the relevant goals, recent failures, evidence, budgets, worker availability, and task dependencies for each decision. Routine heartbeats/lease transitions should not trigger global deliberation. Periodic agenda review handles broader goal generation and reprioritization.

### Example: failed experiment

A `task.failed` event identifies a task and trace artifact. Deterministic handling records the failure and links evidence to its goal. The executive asynchronously asks the model to choose retry, investigation, replan or abandon. A validated response may create a diagnostic task and hypothesis; the transaction records those objects plus a dispatch intent. Hermes executes the task; its result arrives as another event.

## Execution, evaluation, and self-modification

- Reuse Hermes initially for workers, task claiming, leases, retries, cancellation, and progress. The adapter is replaceable; the Factory should not couple to Hermes-specific internal tables.
- Tool capabilities enable trace inspection, code changes, simulations, replay, experiment runs and evaluations.
- Evaluations compare candidate behavior against baselines with tests, deterministic recorded-event replay, and headless simulation where applicable. The Factory may develop new evaluations.
- A candidate Factory version is built from source to Wasm and tested in isolation. The Factory cannot overwrite its running module or directly promote itself.
- Promotion flows through the hub/kernel, with configured gates, signature verification, probation, and last-known-good rollback.
- Observability should expose agenda, decisions, evidence, worker state, costs and promotion outcomes, preferably by extending the existing dashboard.

## Database consistency and replay

A database commit and a coglet step/journal commit are **not** automatically atomic. The design must explicitly define how capability calls are recorded, how replay supplies recorded query results/outcomes without mutating live state, and how crash recovery reconciles commits and outbox dispatches. Do not claim exactly-once execution: use durable intent IDs, idempotent consumers, and reconciliation of uncertain outcomes. Database query order and nondeterministic results must be bounded and recorded. A single executive simplifies conflicts but does not remove transaction/replay edge cases.

## Upgrades and schema migration

Every release identifies its Wasm code commit and supported database schema version. Prefer additive/expand-contract changes; keep operational tables especially stable. Before promotion, snapshot the database and run candidate migrations plus representative replay/compatibility tests against an isolated copy. Never trial migrations on live state.

For a live upgrade, quiesce steps, take a recovery snapshot, run approved transactional migrations, then switch code at a safe step boundary. Code rollback is easy; data rollback is not, especially after external side effects. Keep previous code compatible during probation where feasible; defer destructive changes. In v1, **schema migrations require explicit approval**, while ordinary code/prompt changes use normal gates. An incompatible migration needs a maintenance/recovery plan rather than automatic snapshot restoration.

## Implementation sequence and acceptance criteria

1. **Storage capability + Factory schema:** private SQLite backend, short transactions, migrations, object/edge/event CRUD, outbox. Tests: isolation, rollback on failed transaction, optimistic version conflict, FK integrity, duplicate idempotency keys, bounded query results.
2. **Autonomous Hermes-backed agenda:** event handler, context builder, async LLM decisions, validation, outbox dispatch, retries/restarts. Tests: deterministic routine event, stale LLM response, duplicate event, crash before/after commit, dispatch retry, lease expiration, backlog completion after restart.
3. **Goal generation and prioritization:** periodic review and user/worker-originated goals. Tests: budget/priority enforcement, no duplicate goals from replay, deferral and cancellation.
4. **Research/evaluation:** hypothesis→experiment→evidence→knowledge, trace and artifact references, baseline comparisons. Tests: failed experiment triggers investigation, evidence provenance, reproducible replay against recorded responses.
5. **Self-improvement:** candidate build, isolated DB migration trial, gates, promotion, probation, code rollback. Tests: rejected candidate leaves live code/state unchanged, schema incompatibility blocks promotion, rollback preserves post-upgrade legitimate work.

## Open design questions

- Exact DB capability protocol and transaction lifetime under the event-step ABI.
- Durable outbox ownership: Factory DB, host dispatcher, and the atomicity/reconciliation contract.
- Relationship between Coglet's execution journal and the Factory's semantic events (avoid duplicate event sourcing).
- Factory worker capacity/budget policy and detailed Hermes adapter interface.
- Snapshot/backup mechanics for SQLite now and PostgreSQL later.
- Schema migration approval mechanism and minimum backward-compatibility window.

## Non-goals for v1

Distributed scheduling, shared cognitive state across coglets, a graph database, a full ORM, PostgreSQL production deployment, exhaustive dual-backend CI, and autonomous destructive schema changes.
