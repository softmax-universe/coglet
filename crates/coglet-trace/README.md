# Hub trace archive

`coglet-trace::Store` is the hub's local, synchronous durable storage component.
It accepts complete runtime-exported ABI 0.1 trace JSON bytes, not raw internal
runtime WAL files or independently appended journal fragments. No legacy code,
new wire method, network listener, or default remote debug grant is introduced.
A runtime/visualizer backend uses `query` to obtain archive IDs and `fetch` to
retrieve the exact replay bytes; runtime guest re-execution remains
`Runtime::replay` with an independently pinned engine/program.

## Contract

- `Store::open(path, Limits)` creates SQLite tables in WAL mode with synchronous
  FULL commits. Blob and discovery metadata commit in one IMMEDIATE transaction.
  Concurrent processes serialize quota admission. A persisted versioned quota
  policy must match on every reopen; changing policy requires an explicit offline
  operator migration. No automatic eviction, deletion, or overwrite exists.
- `ingest(run, observed_at, bytes)` validates the archive through
  `Runtime::import_trace`, recomputing blob/record/state integrity before commit.
  The returned ID is lowercase SHA-256 of **exact JSON bytes**, not normalized
  JSON or just final state. Whitespace-different archives have different IDs.
  Identical bytes deduplicate; identical discovery tuples are idempotent.
- The coglet index comes from the validated trace. Run ID is hub-assigned opaque
  ASCII `[A-Za-z0-9_.:-]{1,128}` metadata. `observed_at` is a nonnegative trusted
  Unix millisecond timestamp. It must not be taken from runtime logical time.
- Query supports optional exact coglet/run filters, inclusive observed-time bounds,
  an explicit bounded row count, and stable `(time,id,coglet,run)` ordering. A full
  page does not imply completeness; narrow the interval/filter for further results.
- Fetch validates the ID grammar, bounds allocation by stored byte length, checks
  the content digest and revalidates archive semantics before returning bytes.
  Missing, oversized or damaged content fails closed, never repairs silently.
- Default limits: 16 MiB/archive, 1 GiB unique archive payload, 100,000 discovery
  rows and 1,000 query results. These are logical payload/row limits, **not an
  exact filesystem-byte ceiling**: SQLite indexes, pages, WAL, transaction rollback
  space and archive-validation allocations add overhead. Deploy with a filesystem
  quota for a hard disk ceiling. Exhaustion returns an error and retains all
  prior accepted data. Each call is bounded by archive size but is blocking;
  network services must use bounded blocking execution, not the async reactor.

## Trust boundary

The supervisor must provision a private database directory and keep DB/WAL/SHM
files private. The library is a trusted local component, not an authorization
server: its caller authenticates the producer, binds it to the claimed coglet,
assigns run/time, and checks explicit per-coglet export grants before query/fetch.
Possession of an archive hash grants no authority. Archives include private inbox,
heard state and logs. Never expose this API directly as an unauthenticated route.
The frozen hub protocol has no trace-store method: a remote transport adapter needs
separate specification/authorization review, not an invented extension fallback.
Validation proves internal consistency, not producer signatures, completeness,
truth of observed guest output, or independent guest re-execution. Blob/cutoff and
provenance authentication remain the caller's responsibility.

## Executed tests

`cargo test -p coglet-trace`:

- `ingest_restart_query_fetch_independent_replay_has_identical_hash`: real runtime
  recording, SQLite close/reopen, filtered lookup, exact fetch, independent mock
  guest re-execution and equal output/final state hashes.
- `quotas_are_atomic_deduplicated_and_query_is_bounded`: exact-size acceptance,
  idempotence, row and input-size refusal, bounded queries and durable rollback.
- `corrupt_archive_and_corrupt_stored_bytes_fail_closed`: malformed JSON, changed
  final hash, unknown/invalid IDs and externally corrupted database blob refusal.
- `durable_policy_cannot_be_weakened_by_another_writer`: policy mismatch refusal.
- `concurrent_writers_share_the_total_byte_budget`: separate SQLite connections
  racing unique archives under one aggregate budget; only one can commit; negative
  coglet/run/time filters cannot leak matching rows.

These are component tests, not a new hub wire conformance claim.
