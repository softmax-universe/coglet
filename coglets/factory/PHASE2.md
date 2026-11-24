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
- Host dispatcher talks only to a replaceable Kanban adapter. `HttpKanban`
  uses the authenticated Hermes Board service contract documented below. Tests
  use `MockAdapter` or the recorded-schema `ContractKanbanServer`. Outcomes and replayable event rows commit
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
   The phase-2A `factory_wasm_*` tests compile the actual WASI guest and run it
   on WAMR: `task.failed` → durable `coglet-llm` / `MockLlm` reply → authenticated
   boundary fixture → `runtime.inbox` → diagnostic + hypothesis + dispatch intent.
   Runtime/DB and gateway reopen between boundaries; stale context is refused,
   duplicate provider delivery cannot append again, and offline recorded-result
   replay runs after deleting the DB. Router authentication is supplied by the
   embedding fixture, not qualified by this test (the receiver owns that gate).
   Database live/replay hosts reuse the restricted transactional WASI implementation;
   the immutable inbox and its wake binding retain ABI 0.1 semantics. No new import,
   wire method, schema migration or direct reply-injection seam is introduced.
   The remaining phase-2B gate is outbox dispatch → persisted result → Wasm guest
   consumption, plus authoritative Hermes public API compatibility.
2. Guest does not write `work_queue`. It can read one row through
   `work_queue_get`. The host dispatcher remains the scheduler.
3. No waiver is claimed. The implementation review is not ship approval.

## Phase 2B: authoritative Hermes public contract

**Pinned sources, not an invented REST API.** The Hermes client reviewed is
`softmax-universe/daveey-hermes` commit `ca4035ee64065498d80d80dc2521ed1d82d73cfb`.
The service it calls is `daveey/hermes-hub` commit
`7f66691722b7efceaaf24d9fe766eb60057d932f`. All service file:line references below
are relative to `worker/src/` at that commit. Immutable source links:

- [Hermes client](https://github.com/softmax-universe/daveey-hermes/blob/ca4035ee64065498d80d80dc2521ed1d82d73cfb/hermes_cli/kanban_sync_client.py#L433-L450)
- [Board HTTP routing](https://github.com/daveey/hermes-hub/blob/7f66691722b7efceaaf24d9fe766eb60057d932f/worker/src/index.ts#L338-L489)
- [Board operation definitions](https://github.com/daveey/hermes-hub/blob/7f66691722b7efceaaf24d9fe766eb60057d932f/worker/src/transitions.ts#L973-L1144)
- [Official user documentation](https://hermes-agent.nousresearch.com/docs/user-guide/features/kanban)
  describes the user-facing board, not a `/v1/tasks` HTTP service. The source
  client and service, not that hypothetical endpoint, define the wire contract.

| Factory operation | Authoritative public operation and schema |
| --- | --- |
| Create | `POST /v2/boards/{board}/ops`, body `{ops:[{client_op_id,op:"create",task,fields:{title,body,assignee,created_by,idempotency_key,workspace_kind,review_required}}]}`. `transitions.ts:973-1027`; operation dispatch `1490-1517`; envelope `board.ts:476-637`. Response `{results:[{client_op_id,status,row,...}],head}`; HTTP200 alone is not acceptance. |
| Claim/lease | `POST /v2/boards/{board}/leases`, `{action:"grant",task,run_uid,ttl_s:60,lane:"ready",client_op_id}`. `board.ts:847-937`; Hermes `hermes_cli/kanban_sync_client.py:1966-1988`. Response must be `status:"applied"` with matching task, run and authenticated gateway holder plus unexpired `lease.expires` (epoch milliseconds). Registry placement, spend and source-custody gates remain enforced by the service. |
| Heartbeat | Same `/leases` endpoint, `action:"renew"`, same task/run, TTL and a heartbeat-specific operation ID. `board.ts:927`, client `kanban_sync_client.py:1990-1991`. A new heartbeat gets a new ID; a replay of that heartbeat reuses it. Saved claim recovery renews the saved token, not a new attempt. |
| Retry | **Unsupported, fail closed without I/O.** The exhaustive public semantic operation table `transitions.ts:1490-1517` has no retry operation. `release`, `unblock`, and `request_changes` have different lifecycle/policy semantics and are not substitutes. `MockAdapter` retains its independent retry simulation; HttpKanban does not claim remote retry coverage. |
| Cancel | **Unsupported, fail closed without I/O.** The same exhaustive operation table has no cancel. `archive`/`delete` are administrative operations, not a promise that worker cancellation completed; they are not mapped to cancel. |
| Progress/comment | `/ops`, `{op:"comment",task,run_uid,author,body,uid,client_op_id}`; `transitions.ts:1066-1077`. Comments are append-only; the service does not lease-fence them. Adapter first checks current task/run/holder by exact evidence; that read and comment are **not atomic**. Stable comment UID deduplicates after the operation-cache retention window. |
| Result | `GET /v2/boards/{board}/evidence?table=task_runs&uid={run_uid}`. Client `kanban_sync_client.py:447-450`; service `board.ts:1241-1253`. Check board/table/UID/task bindings, `ended_at`, status and outcome. `done/completed` is success, `crashed/crashed` or `failed/failed` is failure; unknown outcomes refuse. A task's later attempt cannot complete this one. |
| Worker completion (external producer, not Factory guest) | `/ops`, `{op:"complete",task,run_uid,summary,result,metadata,client_op_id}`. `transitions.ts:1121-1144` fences the holder and writes durable run evidence. Captured fixture uses this operation; Factory polls that exact run rather than inventing a `/result` endpoint. |
| Reconcile | `/evidence?table=tasks&uid={task}`; `board.ts:1241-1253`. Only `never_existed` permits fresh creation. `unknown` after history compaction and `deleted` are held, never interpreted as absence. Exact immutable payload/key must match before reusing a live remote identity. |

Configuration is explicit `HttpKanban::authenticated(base, board, bearer, gateway,
assignee)`. Credentials belong to the embedding host, never Wasm, a fixture or
source control. Bearer authenticates the gateway; the caller-supplied Factory
owner name is **not** a gateway credential. HTTPS certificate validation is on;
cleartext is allowed only for loopback fixtures. Redirects and implicit POST
retry are disabled. Unconfigured `FactoryHost::connect` refuses dispatch.

The remote task is `t_` plus the first 16 hex digits of SHA256 of the Factory
idempotency key. A collision or payload mismatch refuses; archived/deleted IDs
are never reused. Run UID is `factory:{remote_id}:{attempt}`; the durable local
work-queue token is this service run UID. Poll now receives the durable attempt
and token explicitly, so reopening an HTTP client loses no correlation state.
A conservative local deadline is capped by remaining server lease duration;
unsupported retry/cancellation leave the work held rather than invent effects.
Local HTTP lease expiry retains the exact saved attempt for evidence polling and
never requeues it or aborts sibling dispatch cycles. Transport/auth/admission
errors are not worker failures: missing configuration refuses, and non-success
HTTP responses preserve uncertainty rather than publishing `task.failed`.
Heartbeat persistence is capped by the returned server lease deadline.
This is idempotent reconciliation, **not exactly-once worker execution**.

### Recorded wire fixtures and provenance

`host/tests/fixtures/board-contract-recording.json` contains actual authenticated
HTTP request/response bodies captured 2026-11-24 on isolated board
`coglet-factory-t01a20f44e61e630a`, never `default`. It records method/path/body,
response status/body and timezone-aware observation timestamps, plus the service
source revision. No bearer/headers are stored. Task `t_a20f44e61e630a01` and run
`factory-fixture-run-0001` are explicitly synthetic: create → grant → renew →
comment → complete → exact task/run evidence → archive → exact archived,
unleased readback. This is real HTTP contract compatibility, not real agent work.
The initial403 records are retained honestly: urllib's default User-Agent was
rejected by the edge; requests using the shipped `hermes-kanban-sync/1` agent
succeeded. The fake selects successful records by label and HTTP status.

`ContractKanbanServer` implements the cited schema with row/lease/run templates
loaded from those recorded responses. It normalizes only fixture identities and
mutable lifecycle fields, validates semantic ops, requires gateway bearer, and
can commit create then drop its response. It is separate from HttpKanban and has
no `/v1/tasks` route. CI is offline: it does not replay writes to the live Board.

### Wasm dispatch/result consumption and crash boundaries

`factory_wasm_outbox_dispatch_result_consumed` extends the approved phase2A
actual-WAMR scenario: task.failed → durable mock-LLM reply → guest diagnostic,
hypothesis and outbox → host HttpKanban → recorded-schema Board fake → durable
result/delivery → authenticated immutable inbox fixture → actual Wasm task
completion. It reopens at lost create reply, after result commit/before inbox,
after inbox commit/before guest, and after guest commit/before acknowledgment.
The inbox message ID is a deterministic 32-hex hash of the durable delivery ID;
the original delivery identity is still used for host acknowledgment. The guest
validates the existing inbox envelope and accepts only the closed Factory task
event kinds (no recursive arbitrary inbox execution). Offline replay of the
result segment succeeds with the live database removed.

Named gates: `hermes_contract_fixture_create_claim_complete`,
`hermes_contract_retry_cancel_progress`,
`factory_wasm_outbox_dispatch_result_consumed`,
`factory_dispatch_crash_reconciles_exactly_once`. The last means one remote
creation and one local outcome/delivery under replay, **not exactly-once remote
execution**. Reopen fixtures and deliberately lost HTTP replies are not a claim
of whole-machine power-loss coverage. Required exact-head CI, strict Clippy/fmt
and independent review remain mandatory before this slice is accepted.

