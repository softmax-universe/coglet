# Factory Phase 2 — implementation in progress

This branch is **not Phase 2 acceptance** and is not a deployment. Phase 1's
`factory.inspect` and `factory.transact` probes are retained. `wake` also enters
`executive::handle` for agenda domain events and `runtime.timer`/`llm.reply`.

## Implemented so far

- Factory-owned, closed serde decision types for CREATE_GOAL, DECOMPOSE,
  PRIORITIZE, DISPATCH, INVESTIGATE, EVALUATE, REPLAN and DEFER. The kernel does
  not interpret them. At most three operations are admitted per decision;
  creation precedes dispatch, IDs must be fresh 32-lowercase-hex strings, and
  task/dependency, worker and budget policy is rechecked before any mutation.
- One persisted executive record with at most one pending asynchronous request,
  selected context, purpose, expiry and state-version echo. Selected object
  versions and policy are reread on reply; stale proposals are discarded and
  replanned at most twice. Uncertain expired calls are not blindly resent.
- Context rooted at one task/goal, its explicit `context_ids` (at most four), and
  the host-provisioned `factory-policy` object. Context and input JSON are bounded
  to 8192 bytes. Trace artifact references remain in the task and diagnostic
  hypothesis; no global agenda scan is included in the model prompt.
- Durable per-event receipts keyed by the frozen event identity `(source, seq)`;
  identical delivery is a no-op and changed content at the same identity conflicts.
  Backlog entries are separate typed objects, not an expanding prompt/root array.
- Routine heartbeat and lease-expiry handling stages no LLM request. Terminal
  tasks do not regress after late failure events. Dispatch reserves worker
  capacity, marks the task dispatched and uses `(task, attempt)`-derived outbox
  identity, committed atomically with cognitive writes.
- Requests use SDK `Context::llm` with the existing text-provider `input.prompt`
  format. Replies accept the existing provider `output.text` JSON or a scripted
  `output.decisions_json`. The gateway test uses `crates/coglet-llm` and the real
  `coglet-mock::llm::MockLlm`, without provider network access or paid calls.
- Host dispatcher talks only to a replaceable Kanban adapter. Production is
  `HttpKanban` (public HTTP create/claim/heartbeat/cancel/result). Tests use
  `MockAdapter` or `MockKanbanServer`. Outcomes and replayable event rows commit
  in the same transaction as the terminal outbox state. Recovery redelivers
  until acknowledged. Leases are `leased` with an attempt token; expiry returns
  them to `pending` and the same idempotency key is retried. Late results are
  rejected unless attempt and lease match.

## Invocation and policy

The embedding host owns event authentication, grants, pinned catalog/migration
approval and current LLM authorization. It must provision an object with ID
`factory-policy`, type `policy`, and JSON matching `executive::Policy`:
`model`, `remaining_budget` (dispatch-admission count), `available_workers`, and
`max_priority`. Monetary/model exposure remains independently enforced by the
LLM gateway. The model never receives authority to write that object.

Configure `max_events = 1` for agenda invocations: this bounds one runtime step
under the existing 32-DB-call ceiling even when the selected context and proposal
are at their limits. The runtime remains the transaction owner. Every error must
abort the entire guest DB/journal step; no guest commit or host network call is
introduced. No schema migration is added.

## Verification so far

```sh
CARGO_TARGET_DIR=target cargo test --workspace --manifest-path coglets/factory/Cargo.toml --locked -j2
CARGO_TARGET_DIR=target cargo clippy --workspace --manifest-path coglets/factory/Cargo.toml --locked --all-targets -j2 -- -D warnings
CARGO_TARGET_DIR=target/factory RUSTFLAGS='-C link-arg=--max-memory=16777216' \
  cargo build --manifest-path coglets/factory/Cargo.toml --locked \
  --target wasm32-unknown-unknown --release -j2
```

`check.sh`, already called by required Rust CI, discovers these tests. Native
SQLite agenda tests exercise actual repository statements and transactional
rollback/reopen; they do **not** substitute for physical process-death/runtime
journal qualification. The latter remains in the Phase 1 runtime suite and must
also be exercised with executive events before final acceptance.

## Still required before merge

1. Green exact-head required CI for this head, then independent review.
   `wasm_task_failed_stages_llm_and_survives_reopen` is the Wasm executive
   proof for `task.failed` → staged LLM request → reopen. The runtime delivers
   `llm.reply` only as an authenticated `runtime.inbox` wake, and this database
   harness has no public inbox-append seam, so the reply-to-dispatch half stays
   on the native executive test plus the host dispatcher tests.
2. Guest does not write `work_queue`. It can read one row through
   `work_queue_get`. The host dispatcher remains the scheduler.
3. No waiver is claimed. The implementation review is not ship approval.
