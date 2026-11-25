# Coglet ABI — 0.1 (proposed)

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things. The runtime's own dispatcher is called the *event router*.

Decision: [ADR-0006](../docs/adr/0006-coglet-abi.md), [PR #8](https://github.com/softmax-universe/coglet/pull/8), amended before freeze by [ADR-0009](../docs/adr/0009-spec-v0-freeze.md).
Imports [common conventions](conventions.md) unchanged; MUST/SHOULD are normative.
Core Wasm, not the component model. Engine budget and determinism requirements
are specified below; real implementation qualification remains fail-closed under
the [owned M1 gates](../docs/reviews/spec-v0-m1-gates.md). This document's presence
on a proposal branch is not final freeze acceptance.

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

LLM requires the granted built-in capability `llm.complete`, routing model name,
id, correlation_id and profile-map input. The exact operation and upstream adapter
boundary are specified in [capability bindings](capabilities/bindings.md); this is
not an adapter action or an implicit router RPC. Success/failure arrives in a later
inbox message whose event kind is `llm.reply`, payload is llm-reply and request_id
matches the request id. Correlation id is retained. Provider timeout/cancellation
is a recorded failure reply. Spend policy belongs to the gateway, not this ABI.

WASI preview1 file operations are restricted to preopens /code, /data, /logs at
fds 3,4,5 respectively. args/environ are empty; proc_exit aborts. stdin is empty;
stdout/stderr append bytes to /logs/stdout and /logs/stderr. No sockets, clocks,
random_get, poll_oneoff, arbitrary host files or environment. Directory entries
sort by UTF-8 bytes, timestamps are zero and host device/inode metadata is hidden.
Adapter state comes through recorded events, never live device imports in 0.1.

## Batching and filesystem

Serialize steps per coglet. Step starts at zero and increases for each durable
attempt, including failure. At admission assign a durable arrival ordinal; select
the oldest prefix up to max_events without comparing cross-source clocks. Events
arriving during execution stay queued. Backpressure on overflow; never silently
drop events. The recorded batch is the replay ordering authority; arrival ordinals
are durable event-router metadata, not reconstructed from source clocks by replay.
Record the exact batch, a nondecreasing runtime-epoch microsecond
logical_time and seed. Preserve source event clocks unchanged. Replay uses the
recording, not a new scheduling decision. An authorized router manage.step with no
queued event injects one recorded common `runtime.manage_step` event: source is
the target coglet, seq uses that source's shared durable sequence, logical_time
is the step's runtime logical time, payload is `{}`, and correlation_id is the
router operation ID. It bypasses subscriptions like runtime.inbox/runtime.timer.
With pending events, manage.step consumes the ordinary batch instead; it never
calls handle with an empty batch. Router authorization/receipt/audit binds the
operation; an external producer cannot self-authorize a runtime-generated wake.

Apply authenticated manage writes/inbox appends between steps only, in recorded
order. Pin code, manifest, grants and input snapshot before instantiation.
Each boundary change includes actor, audit_id (router operation_id or runtime
materialization ID as defined below), nullable
ticket, method, target coglet, path and before/after bytes (null means absent).
The enclosing step or standalone boundary-record supplies before/after tree hashes; a recorded
boundary change is the successful apply outcome. Rejected router operations belong
to the router audit, not the guest mutation list. Initial_hash is before boundary changes; input_hash is after them.
Boundary reconstruction must produce the recorded input (or trailing output) snapshot. Method names
match router wire methods: hub.inbox for inbox appends and manage.write for manage
writes. Runtime terminal receipt materialization uses `runtime.receipt` (not a router
RPC): actor=target coglet, ticket=null, path=/data/inbox.jsonl. It may only append
terminal action messages. Journal origin is structural: this method is origin
`runtime`; `runtime.heard` is also runtime-origin as defined below; other boundary
changes are origin `hub` (including router-relayed provider
replies); guest input-to-output writes and calls
are origin `coglet`. Exporters MUST preserve or explicitly materialize that origin,
never flatten both into unattributed state deltas. The boundary audit_id equals
the router operation_id for router-origin changes; runtime.receipt allocates a stable
materialization audit ID retained across retries. Actor/ticket/target are unchanged.
The M-C amendment also defines `runtime.heard` below as a runtime-origin boundary
mutation, not a router method or guest event. All other origin rules remain unchanged.
The enclosing record and its before/after hashes identify the boundary; only applied
mutations appear here, with failed/queued outcomes retained in router receipts/audit.

- /code: read-only pinned revision, including prompts and step.wasm.
- /data: private read-write persistent state, never git-synced; inbox exception below.
- /logs: create/append only, including log imports; no truncate, unlink or rename.

Paths are absolute UTF-8 with no C0 controls (U+0000–U+001F), DEL
(U+007F), backslash, empty, dot or dot-dot components or trailing slash.
No Unicode normalization is performed. Router manage paths are a bounded ASCII
subset of these filesystem paths, not a second filesystem namespace. Only regular files; roots implicit. Symlinks, hardlinks, device nodes and
mount escapes forbidden. Mount permissions apply to descriptors too, including
open/rename operations; no chmod escalation. Denial returns WASI permission errno
without change. Reads see this step's copy-on-write overlay.

Filesystem state hashes use the Git SHA-256 tree object id defined below
(ADR-0035), not the CBOR encoding of the snapshot map. `.git` components remain
ordinary ABI names; Git checkout/index/fsck restrictions do not alter ABI paths.

Success atomically commits files, input consumption and durable journal/outbox
BEFORE any dispatch. Abort discards all guest writes/actions/timers/LLM requests;
output_hash equals input_hash. Boundary interventions survive guest abort.
Failure consumes the batch only once its failure record is durable; retries are
new explicitly recorded attempts. Never automatically rerun a failed live batch.

Local commit is crash-durable, e.g. fsynced WAL plus content-addressed blobs.
Periodic flush checkpoints committed trees; reclaim WAL only after snapshot and
metadata fsync. fd_sync neither commits a step nor dispatches effects. Recovery
selects the last complete durable commit, never partial files/JSONL lines. Journal
storage is outside guest mounts. Logs feed the durable ordered manage.tail_log stream (hub protocol 1.0),
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
Consumers MUST decode and validate each embedded inbox-message and its event,
including the llm-reply schema for llm.reply, ABI action-rejection for
runtime.action_rejected and common action-outcome for action.outcome (with
matching correlation_id), not just the hex container.
Canonical JSONL validation compares to sorted-key, no-space serialization with
literal UTF-8 Unicode, lowercase `\\u00xx` for otherwise unescaped controls,
short escapes for backspace/tab/LF/form-feed/CR, and escapes only quote/backslash
otherwise. No CRLF, blank lines or alternate whitespace/escape spellings.

Every append produces one runtime.inbox event, payload `{index,id}`, retaining
the enclosed event's correlation_id. Batch contains that wake, not a second copy
of the enclosed event. Steering, factory and LLM replies all use this mechanism;
enclosed event.kind distinguishes them. The coglet maintains its own cursor file
in /data; aborted cursor writes roll back. No truncation/rotation in 0.1; storage
exhaustion backpressures instead. Failed guest steps do not remove inbox lines.

## Guest terminal action delivery

A locally staged emit is not an adapter acceptance. After commit its complete
common action enters the durable pending-action set. A terminal dispatch/adapter
rejection is delivered as an inbox event `runtime.action_rejected`, with the
common `action-accept` constrained by ABI `action-rejection` to accepted=false
(and required error). Its source is the target coglet, generated by the runtime.
Accepted actions deliver one common `action-outcome` as inbox `action.outcome`;
retain the original authenticated adapter source. A legitimately synthesized
runtime outcome uses source=target coglet under adapter/router authority rules;
a source string alone never authorizes synthesis. Authorization must be verified
against retained adapter/router admission audit by the archive consumer; the ABI
trace's semantic checker proves causal/pending-state and structural source binding,
not transport authentication or grant/private-owner authority. Case-contract
vectors with synthesized sources assume that separate authority gate and do not
qualify it. Both payload and envelope
correlation_id MUST equal the pending action's correlation_id, and payload id
MUST equal its id. Positive accepts are upstream bookkeeping, not guest terminal
messages. A rejection produces no outcome.

Only `runtime.receipt` may materialize these terminal inbox messages. The event router
MUST NOT also enqueue the enclosed receipt/outcome as a direct guest event.
Terminal messages bypass guest subscriptions through the ordinary runtime.inbox
wake, whose source is the target coglet (as for runtime.timer). Terminal
materialization atomically appends bytes, consumes the pending action and queues
one wake, BEFORE acknowledging delivery. Durable dedup/recovery maps upstream
retries to that same append; a new inbox ID or source seq does not permit a second
terminal message. Conflicting/repeated terminal attempts fail closed without a
new append. Retain upstream retry receipts and the action lifetime ID ledger;
this does not promise exactly-once physical effects. Aborted emits never become
pending and cannot receive terminals. A later failed guest wake consumes its
wake but does not restore the pending action or erase the immutable inbox line.
Replay checks all these transitions and never dispatches or synthesizes results.

## Runtime-authored heard-history journal record (ADR-0009 M-C)

`runtime.heard` is one boundary-change at the reserved path
`/data/runtime/heard.cbor`, actor=target coglet, ticket=null, with a stable audit_id
retained across retries. Its before/after hex bytes encode core-deterministic CBOR
`heard-state` (ABI schema), a complete private snapshot of per-item alignment and
heard projection. Absence means empty history; after MUST NOT be null. This file
is guest-read-only and cannot be changed by manage.write/hub.inbox/guest writes;
only the runtime may replace it at a boundary. It is not an inbox append and
queues NO wake, guest event or engine invocation. An ordinary later step can read
it; it must be durable before that step consumes conversation history. It may
also occur in trailing_boundary, including zero-step exports. Thus it fits the
closed applied-mutation format above without adding an event or untyped header.

Live construction from owner reads is specified in
[speech heard-history](capabilities/speech.md#heard-only-conversation-history-adr-0010) and
[ADR-0009 M-D](../docs/adr/0009-spec-v0-freeze.md#amendment-m-d-heard-state-construction-from-owner-reads).

`heard-state` contains `items`, ordered by (surface,item), unique per pair. Each
`heard-item` carries surface, item, duration_ms, played_ms, stopped, ordered spans
`{end_ms,text}` and the derived `heard` text. Span ends strictly increase within
duration; played_ms cannot exceed duration. Concatenated span text is alignment
evidence, NOT conversation history. The heard text is exactly the concatenation
of spans ending at/before played_ms, capped to 16,384 UTF-8 bytes without splitting
a character. Missing alignment is an empty spans array and empty heard text.
Duration is the known media extent at that recording, not a prediction of final
stream length. For a retained item duration cannot decrease and alignment is
append-only (existing spans never change); later measured alignment may extend
an empty/partial list. Positions never regress,
and stopped items cannot advance or become live again. A duck snapshot remains
live and may later advance; queued items have played_ms=0 and no heard prefix
(span ends are positive). Previously finished items remain independent. Items
are not removed from the journal snapshot; revocation denies future live access
and archive access follows transcript privacy, never rewriting prior audit bytes.

Validate decoded state shape and these semantic invariants at initial_files and
every mutation, including its runtime origin and read-only mount. Export includes
the exact initial file and each before/after state in ordinary hashed records.
Replay rebuilds history solely from those journal bytes, with no TTS, adapter
read, oracle-injected projection or rewritten intent. This establishes an archive
contract, not authenticated/native alignment or persistence qualification (M1).

## Budgets and engine qualification

Positive per-step limits: compute with explicit engine-specific compute_mode,
linear memory pages (64 KiB), wall_time_us, max_events and aggregate host_bytes.
Zero never means unlimited. Record effective limits; lowering manifest limits
requires explicit recorded admission. Reject unsupported modes before execution.

One admitted attempt has ONE compute allowance and ONE absolute wall deadline,
covering initialization, `abi_alloc` and `handle` together. Before entering each
subsequent call, the qualified engine MUST enforce the exact remaining allowance,
not reset it to the original step limit. Exhaustion prevents further guest calls.
The absolute deadline is established before initialization and is never extended
by allocation, export transition or host work. Record aggregate observed usage;
all failed attempts retain the existing atomic-abort semantics. A WAMR mode must
qualify exact consumption/remaining-budget accounting and positive signed-i32 FFI
limits (no zero, overflow or truncation), or reject admission before execution.
The spike's per-call setter is evidence, not proof of this cumulative contract.

Compute units and safe points are engine/build-specific, not portable fuel;
separate host-memory/stack bounds are required (linear pages do not cap total RSS).
These are explicit [engine M1 qualification gates](../docs/reviews/spec-v0-m1-gates.md),
not unspecified v0 behavior. The budget fields
are engine-neutral: compute_mode selects a qualified engine/build accounting
policy, not a WAMR API name; a future fuel/epochs engine fits without new fields.
Compute caps AND deadlines require an interpreter or another qualified metering
engine. For v0 WAMR, admit only the metered interpreter, including on the Mac and the router.
Ordinary and shared-memory AOT ignore instruction metering; ordinary AOT also
ignores the terminate call. Shared-memory AOT's separately qualified deadline-only
path is NOT admission for this ABI (which requires both limits and unshared memory).
Reject unsupported modes before instantiation, never silently downgrade limits.

A module whose initial OR declared maximum memory exceeds the granted page
budget MUST be refused before instantiation; the runtime MUST NOT silently raise
the cap to the module's initial memory. An engine page-limit setter that raises
it is insufficient: inspect the module first. Initial memory equal to the cap is
admissible when all other checks pass; growth beyond budget returns -1 without
changing memory size. Admission refusal does not call abi_alloc/handle, consume
a queued batch, mutate state, or create a step journal; the admission API reports
resource_exhausted. Once an admitted attempt begins, failures use the journal
semantics below.
Host calls are bounded/cancellable and never wait on network. Compute, memory
or host-byte exhaustion aborts with resource_exhausted; wall expiry aborts with
deadline_exceeded. Record failure kind and observed usage, with no partial effects.
Wall deadlines are nondeterministic; no cross-engine cutoff equality claim.
Real budget conformance requires the owned qualification gates, not just a shape
schema or a mock reporting the desired usage.

## Determinism and publication qualification

Inputs, boundary changes, seeds, grants, budgets, code, manifest and filesystem
are pinned and recorded. No ambient randomness/time/network/environment. Record
LLM replies verbatim; missing recordings fail closed, never query a live model.
Replay checks call order/results, outputs and hashes, stopping at first mismatch.
Wall-time aborted steps replay as recorded failures, not a cutoff instruction
promise. Engine metadata records build, feature/NaN policy and compute mode.
ADR-0003 R1–R5, including its amended ABI import/recorded-state rule, define the
proposed publication policy under ADR-0009; actual cross-architecture float/NaN
rewrite and opcode-allowlist qualification remain explicit engine/router M1 gates.
Do not hash-normalize arbitrary file bytes. No cross-engine resource-cutoff
equality is promised. Formal ADR status acceptance remains with the freeze owner;
passing test-only identity-transform provenance is not real transform qualification.

## Journal and trace: one format

Schemas: [CDDL](messages/abi.cddl), [JSON Schema](messages/abi.schema.json).
A trace contains version, trace_id, coglet, initial_hash, initial_files,
code_hash, manifest_hash, final_hash, initial_effect_ids, initial_actions,
initial_inbox_wakes, initial_timers, initial_llm_requests, records,
trailing_boundary and blobs. All fields are required, including empty arrays
and null trailing_boundary. initial_files hashes to initial_hash; header code and
manifest pins resolve/validate even if records is empty. `initial_actions` is the
complete set of committed common actions awaiting terminal append, with unique
IDs included in initial_effect_ids.actions and coglet matching the trace. It
excludes actions whose terminal line is already in initial_files, even when its
wake remains pending. Terminal append consumes this pending set; it never removes
lifetime IDs. Pending arguments, IDs and wakes are captured atomically with the
initial snapshot. No live state may fill in missing history.
Initial pending timer/LLM arrays capture requests committed before this recording
began whose timer has not fired / reply has not been appended. They have unique
IDs and each ID MUST occur in the corresponding initial_effect_ids array.
Step counters are trace-local; effect IDs remain unique across the coglet lifetime.

initial_effect_ids is `{actions:[id,...], timers:[id,...], llm_requests:[id,...]}`:
the COMPLETE per-kind set of IDs committed before the recording, including
completed/dispatched effects, not only pending work. Each array is sorted ASCII
and duplicate-free. Empty arrays explicitly mean no history, not unknown history.
Do not expire this ledger while the coglet identity exists. A trace producer MUST
snapshot it atomically with initial_hash and pending work; unavailable history
makes the trace incomplete and ineligible for replay/gates. IDs suffice because
any reuse, including an identical argument, returns conflict; original effect
arguments are not needed to decide that return. A replay runtime seeds its ledger
from this field, never from an unavailable live runtime.

For each step, copy that ledger into a staging set. Each successful typed
emit/timer/llm call reserves its ID immediately; a subsequent same-kind call in
the step returns 4. Rejected calls reserve nothing. Commit merges staged IDs;
abort discards reservations. Completion, timer firing and LLM replies do NOT
remove IDs. A recorded 4 without a prior same-kind ID, or 0 with one, is invalid.
Replay MUST calculate/check the result, not inject the recorded return. Validate
argument shape and permissions before dedup, and dedup before resource admission.
This ledger is host state, not part of the filesystem tree hash; retain it in the
trace header and bind the entire header with any authenticated archive/trace-store
receipt (record_hash alone authenticates neither this header nor its provenance).

initial_inbox_wakes is the ordered, unique ascending list of indices already in
the initial inbox whose runtime.inbox events remain queued. All other initial
lines have consumed wakes, regardless of the guest cursor. New boundary appends
add their indices to this queue. A batch may consume an oldest prefix of these
wakes interleaved with other events, leaving the rest pending for later steps or
the next trace. Verify each consumed wake's exact `{index,id}` and correlation_id
against its line; reject nonexistent, repeated, out-of-order or mismatched wakes.
An aborted durable attempt also consumes its wakes. A finite trace may end with
pending wakes; it must not invent delivery to drain them. LLM pending requests
are consumed when the reply is appended, not when its wake is delivered.
Promotion carries this queue and the effect ledger forward, not merely the tree.
Journal streams frame each deterministic-CBOR step-record or boundary-record
with u32be byte length; archive JSON is a fixture view. Their closed shapes
are disjoint (boundary-record has no step/events/calls/engine/usage/status).
Storage policy bounds frame size. A boundary-record seals an exported trace
segment; later steps start a new trace from its final snapshot and derived pending
state. Do not rewrite sealed records or duplicate their mutations. Each step record
retains exact events, boundary audit, input/output snapshots, code/manifest hashes,
effective grants/budgets, engine metadata, ordered calls/results, committed actions,
timers, LLM requests, logs, usage and status/failure. Replies are inbox bytes.

Snapshots map absolute paths to lowercase even-length hex bytes; empty hex is an
empty file. Include /code, /data and /logs, exclude journal storage and implicit
roots. `tree_hash` is the Git tree object id in SHA-256 object format
([ADR-0035](../docs/adr/0035-state-hash-git-tree-id.md)). Remove each path's leading
slash; files are mode `100644` blobs and implicit directories mode `40000` trees,
with no other modes. Object ids are SHA256(`kind + " " + decimal_byte_length + NUL
+ object_body`). Blob bodies are raw file bytes. A tree body concatenates entries
`mode + " " + UTF8(name) + NUL + raw_32_byte_object_id`, in byte order with tree
names compared as if followed by `/`. No Unicode normalization or host metadata
is included. Empty snapshots use the SHA-256 empty tree id. State ids are 64
lowercase hex outside tree objects, including `initial_hash`, `final_hash`,
`before_hash`, `input_hash`, `output_hash` and filesystem `state_hash` fields.
The snapshot map remains the wire/archive representation, not the hash input.
record_hash=SHA256(CBOR(record without record_hash))
for both closed, disjoint record shapes; there is no extra hash domain prefix.
previous=null at step 0, otherwise the preceding record_hash. Trace begins at
step 0, snapshots chain exactly, and initial_hash is the first pre-boundary tree
hash. Successive step input is prior output plus recorded boundary interventions.

`records` may be empty. `trailing_boundary` is null if no later durable change
exists; otherwise it is a boundary-record with version/trace_id/coglet matching
the header, previous=last step record_hash (or null with zero steps), nondecreasing
logical_time (with zero steps, there is no earlier step time to compare), nonempty ordered boundary changes, before_hash, output_hash,
output_files and record_hash. before_hash MUST equal the preceding output tree
(or initial_hash); applying every exact before/after mutation MUST produce
output_files and output_hash. Mount, append-only inbox/log, embedded message,
pending-work and provenance rules apply just as at a pre-step boundary. It has
no guest execution and consumes no queued wake or step number. `final_hash`
MUST equal the trailing output_hash, else last step output_hash, else initial_hash.
A stopped/paused export MUST include all applied mutations through its durable
cutoff, even if no guest step has ever run. No future step may be invented or
required to make the export complete. Neither hashes nor shape validation prove
that the producer disclosed its full durable log: the authenticated archive
receipt binds cutoff/completeness and the entire header, including ledgers and
the trailing record. An initial/trailing boundary may leave pending work for
the next segment; promotion carries the derived pending sets, not only files.

blobs maps lowercase SHA256(raw bytes) to hex bytes. It must resolve every code_hash
(exact step.wasm) and manifest_hash (deterministic CBOR manifest), verified against
manifest schema. All /code auxiliary files are in snapshots. Identity/signatures
are separately verified at admission; hashes are not signatures. A consumer must
retain all content, never only expiring URLs or hashes.

Every pinned `/code` snapshot MUST include `/code/build/provenance.cbor`, the
unchanged core-deterministic CBOR `hub-build-record` from the admitted archive
(see [router publication](hub-protocol.md)). Replay validates that shared schema and
requires `rewritten_sha256 == code_hash == SHA256(/code/bin/step.wasm)`; missing,
malformed, noncanonical or mismatched provenance fails closed before execution.
Provenance stays unchanged for the trace's pinned code revision. Do not replace
it with a new record naming the same output. This binding is not signature or
transform qualification: the admission audit retains the signed archive statement
and qualification evidence, while the build service retains original/recipe/
policy/tool bytes for build reproduction. Pure replay neither recompiles nor
requires a live build service. Fixture identity transforms are test-only, not
production policy acceptance.

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
the pinned manifest limits or change compute_mode. Runtime-generated inbox/timer/
manage_step wakes bypass subscription filtering, but external events must match subscriptions.
proc_exit/nonzero handle map to guest_return; memory access traps to trap; invalid
host buffer addresses to invalid_argument. Wall failures use deadline_exceeded;
compute/memory/host_bytes use resource_exhausted.

## Manifest and conformance

[manifest.schema.json](manifest.schema.json) requires version=0.1, id, exact
subscriptions, required_capabilities, exclusive_outputs and budgets. No duplicates
or wildcards. `required_capabilities` names individual operations (events, reads
or actions), NOT capability entry/group names. `subscriptions` names advertised
event operations. `exclusive_outputs` names advertised exclusive resources;
resources and operations are independent namespaces, so it is NOT a subset of
`required_capabilities`. For example, `speech.speak_chunk` requires its operation
grant and the `audio.speaker` lease, not an `audio.speaker` operation grant.
Before start resolve every requested operation/event/resource through the trusted
capability catalog and require the matching current operation grants and resource
leases. Unknown names fail closed; resource ownership does not imply operation
permission, or vice versa. Manifest id must match runtime identity. Recheck before
dispatch; lost grants deny effects. Guest ABI 0.1 has no synchronous adapter read
import: a requested read operation does not add one; native runtime reads remain
adapter-protocol operations and guest observations arrive as recorded events.

Fixture tests check both schemas, golden CBOR, tree/journal hashes and semantic
negative cases: `python3 -m unittest discover -s spec/tests -v`. Schemas enforce
shapes, not temporal rules. Fixtures are in spec/fixtures/abi. Executable data-only
conformance timelines in conformance/abi use the harness send/expect/journal_hash
interface; their scripted step input contains files, batch and host calls, and
expected output contains the full journal and hash. Real and mock adapters must
execute those cases; fixture validation alone is not implementation conformance.
