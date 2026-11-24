# Factory DB guest and runtime qualification — in progress

ABI 0.2 is explicitly opted in. Frozen ABI 0.1 loaders still reject
`coglet_v0.db`. The borrowed runtime service path now executes DB steps, publishes
full-state decisions through the WAL, and replays recorded DB results offline.
**Milestone D is not yet qualified or landed:** hosted exact-head required CI,
and final independent review remain gates. No live deployment,
production operator-approval adapter or external dispatcher is certified here.

## Repository and schema

- Portable schema source `migrations/0001.sql`: objects, relations, semantic
  events, work_queue, outbox and schema_migrations. Semantic JSON is opaque text;
  outbox payloads retain canonical profile-CBOR identity. No guest SQL or DDL.
- Repository `get_object`, `query_objects`, `get_relations`, `create_object`,
  optimistic `update_object`, `append_event`, and dedicated `enqueue_outbox`.
  Replies are provisional until host step completion; propagate every error.
- Bounded pages retain host revision and authenticated opaque cursor. Application
  ULIDs use supplied timestamp/entropy; ABI action/correlation IDs remain
  32-lowercase-hex. No ambient time/randomness, LLM or executive.
- `factory.inspect` reads one object. `factory.transact` is a deterministic probe,
  not a product executive: supplied IDs/time select get/query/relations, optimistic
  update, semantic event append and outbox enqueue in one host step. Its payload
  reports bounded first-page peer/relation counts, not exhaustive scan totals.

## Actual runtime tests

From repository root, with the pinned toolchain and CMake installed:

```sh
rustup target add wasm32-unknown-unknown
cargo test --locked -p coglet-runtime --features native-engine \
  --test factory_database --test database_service -- --test-threads=1
```

`real_wamr_runs_actual_factory_guest_end_to_end` compiles this Rust guest for
`wasm32-unknown-unknown` with a 16 MiB maximum, checks that imports are exclusively
`coglet_v0.*` (no WASI), and applies the existing NaN-canonicalization qualification
rewrite. It uses the real Factory catalog and host-approved migration flow,
including snapshot, isolated trial restore and code/schema compatibility pins.
The migration runner owns `schema_migrations`; the test omits that table's DDL
from the portable schema source before submitting the migration.

`mock_runs_factory_repository_against_real_database_host` runs the same
`factory::wake` via native mock imports against real SQLite, not canned replies.
Both tests assert read-derived outbox payloads, exact mixed DB/log call order,
committed SQL rows, whole-unit rollback after a duplicate semantic event,
durable reopen/continuation, and offline guest replay with the DB file deleted.
Operator approval/restore verifiers are explicit test fixtures. Application rows
and relations are host-provisioned test setup; this is not a deployment service.
The build is cached per test binary in `target/factory-unknown`.

`database_service` separately exercises real/mock marker-present/absent recovery
without guest execution, inaccessible-DB holds, exact WAL publication and
recorded-result replay with missing/extra/changed-call and poison controls.
Both adapters now also exercise WAL-safe backup, isolated restore, missing
reference refusal and actual recorded-result guest replay. Exact Factory records
roundtrip through 0.2 archives. The full eight-case frozen wire corpus passes
locally on both adapters; see the detailed
[requirement matrix](../../crates/coglet-runtime/DATABASE_QUALIFICATION.md).
This is not a hosted exact-head CI or final milestone acceptance claim.

## Supplemental guest transport checks

```sh
bash coglets/factory/check.sh
```

This existing script runs SDK/native repository tests and lint, then builds a
bounded `wasm32-wasip1` reactor for a **test-only Python Wasmtime/SQLite
transport**. That path remains supplemental: it does not qualify production
WASI admission, authorization, recovery or dispatch. The runtime test above
instead uses a no-WASI guest without widening any engine import allowlist.

The script additionally invokes `crates/coglet-runtime/tools/check-database.py`:
real/mock native runtime, DB recovery/backup and all frozen wire cases. The
existing required Rust job already invokes this owned script. Its historical
workflow step label still says supplemental; no workflow file was edited here.

## Remaining milestone D gates

- Run the wired qualification checks on the eventual PR's exact head in hosted CI.
- Obtain independent exact-head approval, green required CI and merge the one D
  PR. No ADR-0013 gate is waived. External dispatch remains Factory Phase 2.
