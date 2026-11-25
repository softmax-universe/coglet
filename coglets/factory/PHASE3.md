# Factory Phase 3 — work in progress

This branch is **not Phase 3 acceptance** or a deployment. Phase 2 is the base.

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

## Still required

Periodic bounded agenda review, model-generated goal validation/linking,
deterministic priority and dependency eligibility, per-goal budget reservation,
deferral/resumption, cancellation through the attempt-aware outbox, and stale
model/attempt regression coverage are **not implemented yet**. The three card
acceptance names remain unfulfilled until full native and real-WAMR scenarios
exercise those paths and committed outbox effects. No PR or review approval is
claimed by this document.

Paging constraint for the remaining design: the database prohibits writes in a
step consuming **or minting** a cursor; a committed write invalidates a cursor.
Do not implement the periodic scan by appending a DB receipt or updating the
executive root after a partial page. A bounded read-only scan may persist its
continuation in transactional guest files and schedule a later step; any selected
context and policy must be revalidated before mutation. The approved SQL catalog
also intentionally disallows joins, expressions and LIMIT in read statements.
Do not widen that host grammar merely to smuggle Factory ranking into SQL.
