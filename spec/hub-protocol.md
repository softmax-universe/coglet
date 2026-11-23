# Hub protocol — 0.1

Normative MUST/SHOULD follow RFC 2119. Decision: [ADR-0004](../docs/adr/0004-hub-protocol.md),
submitted first as [PR #7](https://github.com/softmax-universe/coglet/pull/7).
Import [common conventions](conventions.md), including identities, errors, version
negotiation, event/action envelopes and deterministic CBOR; do not redefine them.
The executable message definitions are [CDDL](messages/hub.cddl) and
[JSON Schema](messages/hub.schema.json). JSON is a diagnostic view, not the wire.
This is a new protocol, not compatibility with any prior product protocol.

## Transport and session

The only ALPN is the exact ASCII bytes `coglet/hub/0`. Baby Bear serves this
beside `babybear/rpc/1`; it does not add methods to that ALPN. A device runtime
dials its configured, pinned hub iroh node ID. Either endpoint may open a
bidirectional QUIC stream on the resulting authenticated connection. Each stream
carries exactly one operation: one request, optional transfer frames, one terminal
reply, then FIN from both parties. No datagrams, stream reuse or nested RPC.

Each frame is a four-byte unsigned big-endian length followed by one deterministic
CBOR item. Length is 1..1048576 bytes, excluding the prefix. Reject oversize before
allocating; truncated prefix/body, invalid profile, unknown fields, wrong frame
order or correlation mismatch abort that stream with QUIC application code 1.
Do not interpret trailing bytes as another operation. A well-framed request with
bad arguments receives `invalid_argument`; a valid operation failure receives
`hub-failure` with the common error. Other streams remain usable. A responder
SHOULD send a failure before closing when it can safely identify the request.

Every connection begins with `hub.enroll`, initiated by the device, with common
`version-offer`; a success carries common `version-selection`. Only 0.1 is
supported here. No other operation is admitted before success. The initiator
verifies selection. An already enrolled node sends `invite: null`; this still
negotiates a fresh connection, with no grants expanded. Re-enrollment on an
existing connection MUST offer/select only its selected version. Streams racing
enrollment receive `unauthenticated`, with no side effects. Session identity is
the transport-authenticated iroh node ID, never a request-supplied string.

Requests contain a fresh `correlation_id`, method, target (coglet ID or null for
connection scope), and exact typed params. The correlation ID is also the durable
operation ID for mutations. Replies and all transfer frames retain it. `timeout_ms`
is 1..300000, a responder-monotonic duration starting at request receipt, not a
cross-host wall-clock deadline. Accepted asynchronous mutations may finish after
that stream times out; query `hub.receipt` before retrying. No implicit cancel on
FIN/reset. `hub.cancel` explicitly cancels an unapplied queued operation; it cannot
undo an applied mutation or accepted surface action.

## Enrollment, hosting and authorization

An operator opens an enrollment window and creates an invite with at least 256
random bits (64 lowercase hex characters), a hub-monotonic expiry and a bound
expected device node ID. The hub stores only a hash of the secret. Invite checking,
allowlist insertion, initial exact-method grants and one-use consumption MUST be
one durable atomic transaction. Expired, wrong-node, closed-window and used
invites return `unauthenticated` without identifying which check failed. A lost
success is recovered by the now-enrolled peer using `invite: null`, not by reusing
the invite. Windows default closed; rate-limit attempts per peer and globally.
Do not log invite values or include them in error details.

A runtime hosts many independently keyed coglets on one device connection.
`hub.install` supplies a coglet-key signed `delegation` statement naming the
coglet, device node, hub node, generation, expiry and exact allowed methods.
`signer` MUST equal the node key suffix of the coglet ID. The hub validates this
proof against authenticated device identity and its own pinned identity; the
runtime keeps the same binding. A coglet key MUST NOT be supplied to hub builders.
Local provisioning establishes keys and initial delegation; this protocol does
not transport private keys. Only the currently registered device may relay for
that coglet. Device replacement requires operator-approved generation increase;
revocation immediately fences old sessions/streams. A signature alone cannot
replace an active binding. Expiry is Unix seconds checked by the verifier's trusted
clock; unverifiable time fails closed. Generations are uint64 persistent monotonic
counters scoped by (hub, coglet, purpose); overflow requires re-provisioning.

Grant lookup is `(authenticated peer, exact method, target)` with expiry and
generation. No wildcard/prefix implication and no transitive grant. Effective
authority is the intersection of hub policy, enrollment grants, delegation and
local surface grants. The hub remains the only remote caller of device methods;
external debug clients authenticate to the hub separately, never to the runtime.
A request delivered by the trusted hub is still checked against local method and
path limits. Revocation is checked at admission AND before boundary application,
each chunk and action dispatch. Outstanding reads terminate and unapplied writes
fail `permission_denied`. No default debug/log/state export. A ticket ending
revokes its grants and cancels its unapplied writes; provenance records carry its
`actor` and optional `ticket` even though the device authenticates the hub.

| Method | Initiator → responder | Scope and grant |
|---|---|---|
| `hub.enroll` | device → hub | null; invite or existing allowlist |
| `hub.install` | hub → device | coglet; explicit install grant + delegation |
| `hub.start`, `hub.stop`, `hub.status` | hub → device | coglet; each distinct grant |
| `hub.git.push` | hub → device | coglet; only `refs/heads/hub` |
| `hub.git.fetch` | hub → device | coglet; only published/main or logs |
| `hub.events` | device → hub | coglet; enrolled source + relay binding |
| `hub.actions` | hub → device | coglet; action/capability/surface grants too |
| `debug.read` | hub → device | coglet; read paths/step/log range |
| `debug.write` | hub → device | coglet; bounded `/data` paths only |
| `debug.pause`, `debug.step` | hub → device | coglet; independent control grants |
| `hub.inbox` | hub → device | coglet; append only |
| `hub.health` | device → hub | null; metadata only |
| `hub.receipt`, `hub.cancel` | either | original target; original authenticated caller only |

All hub-originated requests, including status, debug.read and git.fetch,
MUST carry `context: {actor, ? ticket}` at request level, except debug.write
and hub.inbox which carry the same fields in params. Receipt/cancel also carry
context to identify the original actor/ticket scope. The hub authenticates its
client and asserts this provenance; devices intersect peer authority with local
`(actor, ticket-or-absent, method, target)` grants and path/ref limits. Missing
context is invalid; an unknown actor, ended ticket or missing scoped grant is
`permission_denied`. Omitting a ticket selects only explicitly provisioned
non-ticket authority, never the union of that actor's ticket grants. The hub
MUST NOT remove/substitute a client's ticket or actor to borrow kernel authority.
These checks precede status/read disclosure and fetch metadata, and repeat before
each response/data frame. Revocation terminates the stream without further data;
already disclosed bytes cannot be recalled. Receipt lookup cannot cross scopes.
All intervention audits and cancellation checks use that context, never an action
payload actor override. Device-originated events/health use the authenticated
relay binding instead, and enroll uses its one-use invite/allowlist.

`debug.read` and `debug.write` never imply one another or pause/step. A debug
reader needing pause or single-step needs those additional explicit grants.
All methods above are the entire 0.1 method set; unknown methods fail
`invalid_argument`, not extension fallback. Grant provisioning/revocation is
operator configuration, not a remotely self-authorizing method.

The successful `hub-success.result` union is selected by the original method,
not freely interchangeable: enroll → hub-enroll-result; status → hub-status;
read → hub-read-result; events → hub-events-result; actions → common
action-accept; step → hub-step-result; fetch/health → hub-empty; all other
methods → hub-mutation-receipt. Reply shape mismatch fails the operation.
Mutation receipts may be queued or terminal except pause/stop/step/git.push,
which wait for their specified boundary or time out; receipt polling recovers
their state. `hub.cancel` returns the original operation's cancelled receipt;
if already applied it returns `conflict` without changing that receipt.

## Lifecycle and boundary ordering

Install creates a stopped coglet with verified delegation, an empty private data
namespace, and a verified initial signed hub commit (objects must already be in
local quarantined storage from a git push or local provisioning). Install checks
manifest capabilities against local grants and the signed artifact. No compiler
runs on the device. A repeated identical durable operation returns its original
receipt; a different install for an existing coglet is `conflict`. Initial git
push may stage objects for an authorized uninstalled target but cannot run code.
Start checks verified code, capability grants and kill switch; starting an
already running coglet is a no-op. Stop waits for a safe boundary, prevents the
next step and cancels pending actions using the surface's cancellation rules;
it cannot retract physical effects. A hung step is interrupted by runtime
budgets. Paused is distinct from stopped. Start resumes paused operation.
Status returns state (`stopped`, `running`, `paused`, `faulted`), live commit,
last committed step, pending mutation count and last shared error or null.

Boundary processing is serialized per coglet and durable: (1) finish or abort the
current step; (2) append audit and apply queued debug/inbox mutations in receipt
order; (3) verify/apply an admitted hub ref change; (4) pin inputs and code for the
next step; (5) run at most one fresh Wasm instance; (6) journal intents before
dispatch. No writes, code changes or new inbox bytes become visible mid-step.
If the ABI refines journal encoding, it must preserve this ordering/provenance.
A failed mutation is audit-recorded, changes nothing, and does not block later
ones. Queue changes and their receipt/audit records commit atomically with data
so crash recovery cannot append an inbox message twice. Stopped/paused coglets
still process boundary mutations without executing guest code.

`debug.pause` is acknowledged `applied` only once no guest step is running;
it does not pause real-time surface reflexes or the kill switch. `debug.step`
requires paused state, applies pending boundary work, executes exactly one nonempty
event batch, and stays paused, returning the committed step number. If no event
is queued, the runtime records a synthetic common `runtime.debug_step` event
(source = target coglet, next durable source sequence, current runtime logical
time, payload = `{}`, correlation_id = this step operation ID). This wake uses
the same source sequencer as other runtime-generated events, bypasses subscription
filtering and is journaled; no empty batch is passed to the ABI.
Concurrent step requests serialize; no implicit resume. On failure, remain
paused or faulted, never silently run. A step timeout can have committed; inspect
its receipt. `debug.read` uses one committed snapshot, never partially running
state. Its result includes step and data-tree hash. Reads/tails are bounded
ranges; polling is opt-in, no unsolicited private-state stream.

## Git control plane

Each coglet has its own repository. Version 0.1 uses git SHA-1 object IDs (40
lowercase hex) strictly as object identifiers, NOT as signature algorithms.
The transfer is a self-contained git bundle v2 (no prerequisites), max 64 MiB,
encoded as lowercase even-length hex chunks (max 32768 raw bytes each). Validate
bundle header, declared ref/tip, object connectivity and `git fsck --strict` in
quarantine, with hooks, alternates, replace refs and external filters disabled.
Only the requested ref may be imported. A transfer SHA-256 binds the complete
raw bundle; a signed SHA-256 digest of `git archive --format=tar <commit>` bytes
(with no user tar configuration) additionally binds the exact code archive.
Untrusted trees MUST reject symlinks, gitlinks, absolute paths, traversal, `.git`
entries, `.gitattributes` and `.gitmodules` at any depth, and any `data` or `/data`
subtree, including logs transfers. Rejecting attributes prevents archive export
rules from omitting code from the signed SHA-256 archive. Imports MUST
NOT overwrite working state or private data. Reject mismatching artifact digest,
missing `bin/step.wasm`, or invalid Wasm before activation for code refs. Logs
refs instead require a logs-only path allowlist and never require Wasm. No ref fallback.
During import enforce finite configured expanded-byte, object-count, delta-depth,
quarantine-disk and CPU/time limits (defaults: 256 MiB, 100000 objects, depth 50,
256 MiB and 30 seconds). Stop before exceeding a limit with `resource_exhausted`;
remove quarantine without updating refs. Compressed size/credit alone is not a
resource bound.

| Ref | Authoritative writer and verification |
|---|---|
| `refs/heads/hub` | Hub kernel signs `hub-ref` with the pinned **hub iroh Ed25519 node key**; runtime verifies. Only hub pushes. |
| `refs/published/main` | Hosted coglet runtime signs `published-ref` with that **coglet's own iroh Ed25519 node key**, not the device or hub key. Hub verifies delegation/binding and key. |
| `refs/heads/logs` | Runtime batch-commits append-only `/logs`; coglet key signs `logs-ref`. Hub fetches only when explicitly granted. |

For each update, `ref-statement` includes purpose, hub node, coglet ID, exact ref,
old commit (null only for creation), new commit, archive SHA-256 and increasing
generation. The authoritative writer (including the receiver of hub pushes)
MUST compare `old` with its current authoritative tip and require a strictly
higher generation in one durable CAS; mismatch/replay is `conflict`.

Fetch is authenticated snapshot observation, NOT replay of a publisher CAS.
After verifying signer, current delegation, purpose/hub/target/ref, complete
bundle and archive digest, an observer atomically compares the signed generation
with its durable watermark scoped by (hub, coglet, ref):
- No watermark: pin the verified current statement as the baseline.
- Higher generation: accept the current snapshot even if `old` differs from the
  observed tip. For A(gen 1) → B(gen 2) → C(gen 3), an observer at A accepts the
  signed B→C statement and C bundle without retrieving B. `old` describes the
  publisher transition, not the observer's precondition.
- Equal generation: only the identical previously verified statement is a no-op;
  any different statement is `conflict`, even if its new tip matches.
- Lower generation: reject with `conflict`; never rewind the observed watermark.

Recheck against the latest watermark at commit so concurrent fetch completions
cannot overwrite a newer observation. Persist statement, watermark and tracked
ref atomically; reconnect/restart cannot erase anti-replay state. A same-statement
retry still verifies its transfer and current authority. Fetch may skip historical
observations but MUST retain its own accepted-statement audit. It does not prove
possession of every intermediate publication; that is not a 0.1 guarantee.
An exact duplicate operation returns its original receipt instead. No forced ref
rewind: rollback is a new authorized generation whose new commit restores a
last-known-good tree, with correct old ref; signed history never loses audit.
Admission stages hub pushes; their terminal receipt becomes `applied` only at the
boundary after verification. Status exposes the live commit, not just staged tip.
The published ref attests an explicitly published code tree, never `/data` or
unfiltered private state. It need not move with every step. Logs commits contain
only approved log files, no code or data; roughly every 30 seconds or on rotation,
not each step. Debug live log reads do not require a git commit.

Build-then-commit is a hub responsibility: pin source and build recipe/toolchain,
compile core Wasm into `bin/step.wasm`, validate and run gates, commit source,
manifest, prompts and artifact together, then kernel signs the admitted commit
and transfers it. A build failure cannot update `refs/heads/hub`. Factory or
builder output is never a promotion signature. Policy/gates and probation remain
kernel responsibilities, not bypasses granted by `hub.git.push`.

All signed values use Ed25519, lowercase 128-hex signature. Sign exactly UTF-8
`coglet/hub/0:<purpose>\n` followed by core-deterministic CBOR of `statement`,
excluding `signer` and `signature`. Purposes are `delegation`, `hub-ref`,
`published-ref`, `logs-ref`. Verifiers select the expected purpose before checking
and reject foreign hub, target, ref, expired delegation or signer. Domain
separation prevents replay across purposes/protocols. Public signing fixtures
contain test-only seeds; never deploy them.

Git push: request declares size/hash and signed ref statement; responder sends
`hub-credit`; initiator sends numbered `hub-chunk` frames until exactly size
bytes, then `hub-end`. Responder validates, queues update and eventually replies.
Fetch: request specifies ref; responder sends `hub-transfer` metadata and signed
statement, waits for initiator credit, sends chunks/end, then terminal success.
Receiver verifies before accepting/importing. A fetch observer MUST check signed
metadata against its current watermark before issuing credit; an older or
equal-but-different statement is a local `conflict` and resets the stream with
application code 1 without credit. Recheck the watermark after transfer as well.
Zero/missing/trailing chunks,
incorrect offsets, bytes beyond declared size or hash mismatch fail atomically.
No partial resume; retry same operation or start a new fetch. Rejected transfers
have no ref/data effects and quarantines are removed on timeout/cancellation.

## Event and action relay

`hub.events` carries a bounded batch of common events from a hosted coglet.
The source must match the target coglet; surface events are re-emitted with
coglet source and recorded original provenance in payload by the local router.
The hub derives device routing from enrollment/delegation, never payload paths.
Sequences obey common durable monotonicity; a receiver persists its source
watermark and batch digest with admission. An identical batch retry is deduped;
conflicting reuse is `conflict`. A gap returns `conflict` with expected sequence;
never silently omit an event. Saturation returns `resource_exhausted` before
admission; upstream retains reliable events or explicitly records local loss.
No direct coglet-to-coglet connection, including siblings on the same device.
The hub applies routing/policy and creates target inbox entries/events as needed.

`hub.actions` carries one common action. Its coglet must match target. Runtime
checks hub authority, local capability and exclusive output ownership, maps the
executing surface clock, and returns common action-accept in the result.
An immediate rejection is terminal, never an outcome. Admission is not completion;
one eventual common action-outcome is returned as an `action.outcome` event via
`hub.events` (correlation IDs match). Persist `(coglet,id)` envelope and admission
before dispatch. Conflicting reuse fails; identical retry does not re-dispatch.
On restart reconcile admitted actions with surface receipts; when effects are
unknown, report failed with `unavailable` and explicit unknown-effect details,
never blindly replay. No exactly-once physical-effect guarantee. A kill switch
always wins. Expired actions cannot be dispatched even after reconnect.

## Inbox and debug mutation receipts

`hub.inbox` queues one delivery `{id, sender, event}`; event is the unchanged
common event envelope and `event.source` MUST equal sender. The persisted line
is `inbox-message` per [spec/abi.md (PR #8)](https://github.com/softmax-universe/coglet/pull/8),
not a second hub-specific file format. The hub's `hub-inbox-delivery` schema is
only an append request without the runtime-assigned index; it is never a persisted
line. Runtime assigns the next contiguous index and serializes the ABI line
(compact sorted-key JSON, unescaped Unicode and standard JSON escapes), journaling
the exact file JSON bytes,
appends UTF-8 JSON plus one LF at a boundary, and emits the ABI `runtime.inbox`
wake `{index,id}`. Index exhaustion fails closed. Newlines inside strings are
escaped; integers retain full precision. `sender` is authenticated hub identity
(or the hub-validated original source on relay); never trust an arbitrary sender
string. The hub checks sender authority and the runtime trusts only the pinned
hub's explicit provenance. LLM results/steering/factory notes use this primitive.
Runtime owns append; coglet owns its separate read cursor. Neither debug.write
nor guest code may truncate/rewrite the inbox. No embedded inbox import and no
implicit git export. Message `id` dedupes per target permanently, or until explicit
operator reprovisioning; same ID/different message is `conflict`.

`debug.write` replaces a bounded file at a canonical absolute `/data/...` path,
with UTF-8 text content and an expected pre-write ABI snapshot hash. No traversal,
empty components, NUL, backslashes, symlink resolution, inbox or runtime cursor/
receipt/audit internals. Permission/path limits and expected hash are checked
again at application. Failure is atomic `conflict`/`permission_denied`. There is
no arbitrary command execution or `/code` write via debug. Binary debug writes
are not in 0.1. Reads use similarly canonical `/data` or `/logs` paths (and
`@step/<decimal>` for recorded inputs/outputs); grant scopes may narrow them.

A mutation result is a receipt `{operation_id, state, step, error}`. State is
`queued`, `applied`, `cancelled` or `failed`; step is null until applied and error
is non-null iff failed. Queued is durable admission, not delivery. `hub.receipt`
retrieves the latest receipt. Queued operations survive disconnect/restart;
revocation, timeout before admission and explicit cancel are not silent drops.
Identity+operation ID+target bind a mutation to a digest of method, params and
actor/ticket context
(excluding timeout); reuse with changed intent is `conflict`. Enrollment is the
special atomic invite transaction, not a queued mutation.

Durable mutation/action receipts and input digests are kept for the coglet's
provisioning lifetime, with no time-based ID reuse; storage pressure refuses new
admission before side effects. Event dedupe keeps source watermark and digest of
the most recent accepted batch: older batches return conflict, not re-delivery.
`not_found` on receipt means never admitted for that caller/target, not permission
to retry someone else's ID. An authorized caller can retry its original mutation
unchanged. Fetch/status/read/health are observation operations and may be retried
with fresh correlation IDs; action IDs still retain their common meaning.

Each intervention audit record contains operation ID, actor, optional ticket,
method, target, applied boundary step, before/after data hashes and outcome.
Do not include secret file contents. Journal references the audit ID and marks
origin `hub` versus guest changes marked `coglet`; training export MUST preserve
that distinction. Audit is local, durable and append-only outside guest-writable
paths. If audit/receipt cannot commit, do not apply a write or inbox append.

## Health, limits and backpressure

Health is explicit metadata: device node ID, monotonic uptime milliseconds,
queue depth, available bytes, and per-coglet state/step/live commit. No file
contents, prompts or transcript. Device reports every 10 seconds while connected;
a hub marks degraded after 30 seconds without accepted health and disconnected
on transport loss. These are hub-local elapsed durations, not clock comparison.
Health alone cannot authorize enrollment/start. Reconnection resumes no expired
grants, and queued operations are recovered from receipts.

At most 16 concurrent operation streams per peer, including at most 2 git
transfers, and 4 MiB of queued decoded request data per peer. Reserve one stream
slot for stop/cancel/health/receipt. Requests carry at most 64 events; individual
inbox/debug text is at most 65536 UTF-8 bytes. Queue capacity is also bounded per
target to 256 pending mutations; reject over capacity before admission. Reads
return at most 65536 bytes per call; a text slice must end on UTF-8 boundaries.
Wire/schema text character limits are coarse bounds; byte limits are additional
semantic checks. Hub and device advertise no arbitrary unbounded windows.

QUIC credit is necessary but insufficient. Only git transfers have multiple data
frames: receiver sends cumulative `hub-credit.bytes` measured in raw decoded
bundle bytes (initially zero; max declared size, never decreasing). Sender may
send chunks only within credit, offsets start at zero and advance contiguously.
Receiver extends credit only as bounded storage is durably consumed; total
unconsumed allowance MUST stay <=65536 raw bytes. Credit/end/terminal control
frames are exempt from byte credit, bounded by frame/concurrency limits. No
credit within request timeout aborts transfer with `deadline_exceeded`. Reject
credit overflow/decrease/oversend; no wraparound. Event/action/inbox calls use
bounded unary admission, so no hidden infinite queue or streaming subscription.
Unclassified streams have a 2-second deadline to supply a complete request
frame; at most two may be unclassified concurrently. Reset stalled/extra streams
without admitting work. Set QUIC incoming stream capacity to allow classification
even with 15 occupied work slots: slot 16 is provisional until classified; if its
method is not stop/cancel/health/receipt, reject it immediately. Apply this deadline
even before enrollment. Classification and control dispatch have a separate
bounded scheduling budget, not the bulk queue.
Rate limit health/enroll/control separately to prevent bulk-transfer starvation;
fair scheduling MUST service stop/cancel under bulk saturation. A full queue
returns `resource_exhausted`, `retryable: true`, without consuming the operation
ID. Retry with backoff, not a busy loop. Never drop reliable admitted work.

## Fixtures and conformance

Run `python3 -m unittest discover -s spec/tests -v` with
`spec/tests/requirements.txt` installed (Python 3.12 matches CI).
`test_hub.py` validates every request/result/transfer/signing shape against both
schemas, exact golden CBOR, framing negatives and test-key signatures.
[Case data](../conformance/hub/) defines black-box temporal scenarios;
[the suite contract](../conformance/hub/README.md) specifies observable assertions.
Case data/schema checks are NOT a claim that a hub/runtime or mock already passes
wire conformance. The harness owner must execute the same cases against both.
Snapshot hashes and persisted inbox/journal encoding are owned by [the ABI](abi.md):
SHA-256 of core deterministic CBOR of the snapshot path-to-hex map, including
`/code`, `/data`, `/logs`, excluding journal storage. `debug.read.snapshot_hash`
and `debug.write.expected_hash` use that definition. No alternate hash or inbox
format is standardized here.
