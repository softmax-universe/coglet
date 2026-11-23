# Coglet ABI — 0.1 (proposed)

Decision: [ADR-0006](../docs/adr/0006-coglet-abi.md), [PR #8](https://github.com/softmax-universe/coglet/pull/8).
Imports [common conventions](conventions.md) unchanged; MUST/SHOULD are normative.
Core Wasm, not the component model. This is not the v0 freeze: engine budget
and determinism qualifications remain explicitly TBD-by-ADR below.

## Guest exports and imports

Export one unshared wasm32 memory `memory`, with declared maximum, plus:

```text
abi_alloc(len: i32) -> i32
handle(events_ptr: i32, events_len: i32, step: i64, logical_time: i64, seed: i64) -> i32
```

The logical contract is `handle(events, step, logical_time, seed)`. i32 offsets
and lengths and i64 counters are unsigned bit patterns. Events is a nonempty
array of common event envelopes encoded as core deterministic CBOR. abi_alloc
returns a buffer of at least len bytes valid until handle returns (zero is a
valid address). Host validates widened offset+length against current memory on
every call and copies bytes before reentry. Never pass native pointers or usize.
No imported memory, memory64, start section or `_start` execution. Initialization
and allocation share the step budget. Zero return requests commit; nonzero,
proc_exit (even zero), trap or invalid pointer aborts. Discard the instance,
globals, allocations and descriptors after every wake.

Imports from `coglet_v0` all have `(ptr: i32, len: i32) -> i32`:

| Import | Deterministic CBOR argument | Effect |
|---|---|---|
| timer | timer-request | stage one-shot wake at runtime logical `at` |
| emit | common action | stage action; coglet must match running identity |
| llm | llm-request | stage async request, never await provider |
| log | log-entry | stage structured append |

Return codes: 0 staged (NOT external admission/completion), 1 invalid_argument,
2 permission_denied, 3 resource_exhausted, 4 conflict. Others reserved. Rejected
calls stage nothing. Malformed/out-of-range pointers abort with invalid_argument;
malformed CBOR returns 1. The call's raw argument bytes are retained as
argument_hex with result=1 when it cannot satisfy the typed argument schema.
An invalid buffer address aborts before copying; failure.error.details records
op/ptr/len instead of fabricating an argument. Calls are recorded with argument
and return in invocation
order, including rejected calls. IDs must be unique per effect kind (action, timer, LLM request) across committed steps;
guests SHOULD derive IDs from seed, step and call index. Duplicate IDs return 4.
Timers cannot replace/cancel in 0.1; past-due timers wake only after commit, in a
later step. Timer events use `runtime.timer`, payload `{id, at}`, correlation_id=id.

LLM requires a granted capability (usually llm.complete), routing model name,
id, correlation_id and profile-map input. Success/failure arrives in a later
inbox message whose event kind is `llm.reply`, payload is llm-reply and request_id
matches the request id. Correlation id is retained. Provider timeout/cancellation
is a recorded failure reply. Spend policy belongs to the gateway, not this ABI.

WASI preview1 file operations are restricted to preopens /code, /data, /logs at
fds 3,4,5 respectively. args/environ are empty; proc_exit aborts. stdin is empty;
stdout/stderr append bytes to /logs/stdout and /logs/stderr. No sockets, clocks,
random_get, poll_oneoff, arbitrary host files or environment. Directory entries
sort by UTF-8 bytes, timestamps are zero and host device/inode metadata is hidden.
Surface state comes through recorded events, never live device imports in 0.1.

## Batching and filesystem

Serialize steps per coglet. Step starts at zero and increases for each durable
attempt, including failure. At admission assign a durable arrival ordinal; select
the oldest prefix up to max_events without comparing cross-source clocks. Events
arriving during execution stay queued. Backpressure on overflow; never silently
drop events. The recorded batch is the replay ordering authority; arrival ordinals
are durable router metadata, not reconstructed from source clocks by replay.
Record the exact batch, a nondecreasing runtime-epoch microsecond
logical_time and seed. Preserve source event clocks unchanged. Replay uses the
recording, not a new scheduling decision.

Apply authenticated debug writes/inbox appends between steps only, in recorded
order. Pin code, manifest, grants and input snapshot before instantiation.
Each boundary change includes actor, audit_id (the hub operation_id), nullable
ticket, method, target coglet, path and before/after bytes (null means absent).
The enclosing step supplies boundary step and before/input tree hashes; a recorded
boundary change is the successful apply outcome. Rejected hub operations belong
to the hub audit, not the guest mutation list. Initial_hash is before boundary changes; input_hash is after them.
Boundary reconstruction must produce the recorded input snapshot.

- /code: read-only pinned revision, including prompts and step.wasm.
- /data: private read-write persistent state, never git-synced; inbox exception below.
- /logs: create/append only, including log imports; no truncate, unlink or rename.

Paths are absolute UTF-8 with no NUL, empty, dot or dot-dot components or trailing
slash. Only regular files; roots implicit. Symlinks, hardlinks, device nodes and
mount escapes forbidden. Mount permissions apply to descriptors too, including
open/rename operations; no chmod escalation. Denial returns WASI permission errno
without change. Reads see this step's copy-on-write overlay.

Success atomically commits files, input consumption and durable journal/outbox
BEFORE any dispatch. Abort discards all guest writes/actions/timers/LLM requests;
output_hash equals input_hash. Boundary interventions survive guest abort.
Failure consumes the batch only once its failure record is durable; retries are
new explicitly recorded attempts. Never automatically rerun a failed live batch.

Local commit is crash-durable, e.g. fsynced WAL plus content-addressed blobs.
Periodic flush checkpoints committed trees; reclaim WAL only after snapshot and
metadata fsync. fd_sync neither commits a step nor dispatches effects. Recovery
selects the last complete durable commit, never partial files/JSONL lines. Journal
storage is outside guest mounts. Logs may be batch git-committed (~30s/rotation),
not per step; this is separate from local durability. Persistence failure means
no dispatch; recover the commit identity (trace_id,step), do not blindly rerun.
Dispatch committed outbox in call order with stable identities. Receipt/outcome
is a later event, never an edit to old records. Common action deduplication applies.
Outbox durability is not an exactly-once promise for non-idempotent providers;
uncertain LLM delivery must be reconciled, not blindly resent. Replay never dispatches.

## Inbox

/data/inbox.jsonl is host append-only and guest read-only. Every line is one
inbox-message: compact UTF-8 JSON, sorted keys, unescaped Unicode, standard JSON
escapes, no BOM/duplicate keys/floats, terminated by LF. Preserve full integer
precision. index starts at zero, increases by one; id is deduplicated across the
trace. sender is authenticated and equals enclosed event.source. Sequence numbers
are common source seq, independent of index. All appends happen at boundaries;
the exact resulting bytes are recorded (replay does not reserialize JSON).

Every append produces one runtime.inbox event, payload `{index,id}`, retaining
the enclosed event's correlation_id. Batch contains that wake, not a second copy
of the enclosed event. Steering, factory and LLM replies all use this mechanism;
enclosed event.kind distinguishes them. The coglet maintains its own cursor file
in /data; aborted cursor writes roll back. No truncation/rotation in 0.1; storage
exhaustion backpressures instead. Failed guest steps do not remove inbox lines.

## Budgets — TBD-by-ADR-0001

Positive per-step limits: compute with explicit engine-specific compute_mode,
linear memory pages (64 KiB), wall_time_us, max_events and aggregate host_bytes.
Zero never means unlimited. Record effective limits; lowering manifest limits
requires explicit recorded admission. Reject unsupported modes before execution.
Compute units, safe points and total host-memory/stack accounting remain
TBD-by-ADR-0001 (proposed findings PR #6), not portable fuel. AOT must not silently
claim interpreter metering/interruption. Reject modules with initial/maximum
memory above page budget before instantiation; growth beyond budget returns -1.
Host calls are bounded/cancellable and never wait on network. Compute, memory
or host-byte exhaustion aborts with resource_exhausted; wall expiry aborts with
deadline_exceeded. Record failure kind and observed usage, with no partial effects.
Wall deadlines are nondeterministic; no cross-engine cutoff equality claim.
Frozen budget conformance requires the ADR decisions, not just this shape schema.

## Determinism — TBD-by-ADR-0003

Inputs, boundary changes, seeds, grants, budgets, code, manifest and filesystem
are pinned and recorded. No ambient randomness/time/network/environment. Record
LLM replies verbatim; missing recordings fail closed, never query a live model.
Replay checks call order/results, outputs and hashes, stopping at first mismatch.
Wall-time aborted steps replay as recorded failures, not a cutoff instruction
promise. Engine metadata records build, feature/NaN policy and compute mode.
Cross-engine float/NaN transform, opcode allowlist and resource cutoff equality
are TBD-by-ADR-0003 (proposed PR #5); do not hash-normalize arbitrary file bytes.
Both proposed engine findings and the platform ADR were read; neither pending
proposal is silently adopted as frozen ABI here.

## Journal and trace: one format

Schemas: [CDDL](messages/abi.cddl), [JSON Schema](messages/abi.schema.json).
A trace contains version, trace_id, coglet, initial_hash, records and blobs.
Journal streams frame each deterministic-CBOR step-record with u32be byte length;
archive JSON is a fixture view. Storage policy bounds frame size. Each record
retains exact events, boundary audit, input/output snapshots, code/manifest hashes,
effective grants/budgets, engine metadata, ordered calls/results, committed actions,
timers, LLM requests, logs, usage and status/failure. Replies are inbox bytes.

Snapshots map absolute paths to lowercase even-length hex bytes; empty hex is an
empty file. Include /code, /data and /logs, exclude journal storage and implicit
roots. tree_hash=lowercase SHA256(core_deterministic_CBOR(snapshot)). No host
metadata or NaN normalization. record_hash=SHA256(CBOR(record without record_hash)).
previous=null at step 0, otherwise the preceding record_hash. Trace begins at
step 0, snapshots chain exactly, and initial_hash is the first pre-boundary tree
hash. Successive step input is prior output plus recorded boundary interventions.

blobs maps lowercase SHA256(raw bytes) to hex bytes. It must resolve every code_hash
(exact step.wasm) and manifest_hash (deterministic CBOR manifest), verified against
manifest schema. All /code auxiliary files are in snapshots. Identity/signatures
are separately verified at admission; hashes are not signatures. A consumer must
retain all content, never only expiring URLs or hashes.

Committed effect arrays equal successful calls filtered by operation, in order;
on abort they are empty but calls remain diagnostic. log appends sorted-key compact
JSON log-entry plus LF to /logs/events.jsonl. Failure kinds distinguish compute,
memory, wall, host_bytes, trap, guest_return, invalid_argument. engine/usage/error
are host observations, not guest-visible nondeterminism. Gate, visualizer simulated
mode, replay and trace store MUST use this format. Private snapshots stay local;
export requires explicit authorization, and hashes are not anonymization. Path grammar, blob key digests, array uniqueness
and temporal rules are semantic constraints in addition to shape validation;
CDDL and JSON validators alone do not prove them. A trace pins one code/manifest
pair; a promotion starts a new trace from the previous durable output snapshot.
/logs/events.jsonl is reserved for host log imports (guest read-only); other log
files permit raw append. Log calls must append their exact JSON lines, in order.
The explicit effective budgets field records admission limits; it must not exceed
the pinned manifest limits or change compute_mode. Runtime-generated inbox/timer
wakes bypass subscription filtering, but external events must match subscriptions.
proc_exit/nonzero handle map to guest_return; memory access traps to trap; invalid
host buffer addresses to invalid_argument. Wall failures use deadline_exceeded;
compute/memory/host_bytes use resource_exhausted.

## Manifest and conformance

[manifest.schema.json](manifest.schema.json) requires version=0.1, id, exact
subscriptions, required_capabilities, exclusive_outputs and budgets. No duplicates
or wildcards. Exclusive outputs must be a subset of required capabilities.
Before start require subscriptions/grants and exclusive leases; manifest id must
match runtime identity. Recheck before dispatch; lost grants deny effects.

Fixture tests check both schemas, golden CBOR, tree/journal hashes and semantic
negative cases: `python3 -m unittest discover -s spec/tests -v`. Schemas enforce
shapes, not temporal rules. Fixtures are in spec/fixtures/abi. Executable data-only
conformance timelines in conformance/abi use the harness send/expect/journal_hash
interface; their scripted step input contains files, batch and host calls, and
expected output contains the full journal and hash. Real and mock adapters must
execute those cases; fixture validation alone is not implementation conformance.
