# Router protocol — 2.1 (2.0 retained)

Router 2.1 adds the presence methods specified below by
[ADR-0045](../docs/adr/0045-presence-attention-router-wire.md).
The prior 2.0 method semantics and build-provenance version remain unchanged.

ADR-0035 takes its fallback: merged router 1.0 retains its CBOR hash meaning.
Its normative text and schema snapshots are retained in
[`frozen/hub-1.0/`](frozen/hub-1.0/README.md). Router 2.0 is a distinct major;
no negotiation of 1.0 authorizes Git-tree hash semantics.


> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things. The runtime's own dispatcher is called the *event router*.

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
dials its configured, pinned router iroh node ID. Either endpoint may open a
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
`version-offer`; a success carries common `version-selection`. Versions 2.0 and
2.1 are supported here; greatest offered supported version wins. Presence
methods/results and presence method names in delegations MUST NOT be sent or
accepted on a 2.0 session. The ALPN suffix identifies unchanged transport framing, not
the selected message version. Common and surface 0.1 shapes remain unchanged;
there is no negotiation fallback to the removed hub 0.1 method set. No other operation is admitted before success. The initiator
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
undo an applied mutation or accepted adapter action.

## Enrollment, hosting and authorization

An operator opens an enrollment window and creates an invite with at least 256
random bits (64 lowercase hex characters), a router-monotonic expiry and a bound
expected device node ID. The router stores only a hash of the secret. Invite checking,
allowlist insertion, initial exact-method grants and one-use consumption MUST be
one durable atomic transaction. Expired, wrong-node, closed-window and used
invites return `unauthenticated` without identifying which check failed. A lost
success is recovered by the now-enrolled peer using `invite: null`, not by reusing
the invite. Windows default closed; rate-limit attempts per peer and globally.
Do not log invite values or include them in error details.

A runtime hosts many independently keyed coglets on one device connection.
`hub.install` supplies a coglet-key signed `delegation` statement naming the
coglet, device node, hub node, generation, expiry and exact allowed methods.
`signer` MUST equal the node key suffix of the coglet ID. The router validates this
proof against authenticated device identity and its own pinned identity; the
runtime keeps the same binding. A coglet key MUST NOT be supplied to router builders.
Local provisioning establishes keys and initial delegation; this protocol does
not transport private keys. Only the currently registered device may relay for
that coglet. Device replacement requires operator-approved generation increase;
revocation immediately fences old sessions/streams. A signature alone cannot
replace an active binding. Expiry is Unix seconds checked by the verifier's trusted
clock; unverifiable time fails closed. Generations are uint64 persistent monotonic
counters scoped by (hub, coglet, purpose); overflow requires re-provisioning.

Grant lookup is `(authenticated peer, exact method, target)` with expiry and
generation. No wildcard/prefix implication and no transitive grant. Effective
authority is the intersection of router policy, enrollment grants, delegation and
local adapter grants. The router remains the only remote caller of device methods;
external manage clients authenticate to the router separately, never to the runtime.
A request delivered by the trusted router is still checked against local method and
path limits. Revocation is checked at admission AND before boundary application,
each chunk and action dispatch. Outstanding reads terminate and unapplied writes
fail `permission_denied`. No default manage/log/state export. A ticket ending
revokes its grants and cancels its unapplied writes; provenance records carry its
`actor` and optional `ticket` even though the device authenticates the router.

| Method | Initiator → responder | Scope and grant |
|---|---|---|
| `hub.enroll` | device → router | null; invite or existing allowlist |
| `hub.install` | router → device | coglet; explicit install grant + delegation |
| `hub.start`, `hub.stop`, `hub.status` | router → device | coglet; each distinct grant |
| `manage.patch` | router → device | coglet; signed `/code` patch only |
| `manage.tail_log` | router → device | coglet; explicit ordered log stream grant |
| `hub.events` | device → router | coglet; enrolled source + relay binding |
| `hub.actions` | router → device | coglet; action/capability/adapter grants too |
| `manage.read` | router → device | coglet; read paths/step/log range |
| `manage.write` | router → device | coglet; bounded `/data` paths only |
| `manage.pause`, `manage.step` | router → device | coglet; independent control grants |
| `hub.inbox` | router → device | coglet; append only |
| `hub.health` | device → router | null; metadata only |
| `hub.receipt`, `hub.cancel` | either | original target; original authenticated caller only |

All router-originated requests, including status, manage.read and manage.tail_log,
MUST carry `context: {actor, ? ticket}` at request level, except manage.write
and hub.inbox which carry the same fields in params. Receipt/cancel also carry
context to identify the original actor/ticket scope. The router authenticates its
client and asserts this provenance; devices intersect peer authority with local
`(actor, ticket-or-absent, method, target)` grants and path limits. Missing
context is invalid; an unknown actor, ended ticket or missing scoped grant is
`permission_denied`. Omitting a ticket selects only explicitly provisioned
non-ticket authority, never the union of that actor's ticket grants. The router
MUST NOT remove/substitute a client's ticket or actor to borrow kernel authority.
These checks precede status/read disclosure and tail metadata, and repeat before
each response/data frame. Revocation terminates the stream without further data;
already disclosed bytes cannot be recalled. Receipt lookup cannot cross scopes.
All intervention audits and cancellation checks use that context, never an action
payload actor override. Device-originated events/health use the authenticated
relay binding instead, and enroll uses its one-use invite/allowlist.

`manage.read` and `manage.write` never imply one another or pause/step. A manage
reader needing pause or single-step needs those additional explicit grants.
All methods above plus the 2.1 presence table below are the entire method set;
unknown methods fail
`invalid_argument`, not extension fallback. Grant provisioning/revocation is
operator configuration, not a remotely self-authorizing method.

The successful `hub-success.result` union is selected by the original method,
not freely interchangeable: enroll → hub-enroll-result; status → hub-status;
read → hub-read-result; events → hub-events-result; actions → common
action-accept; step → hub-step-result; tail_log → manage-tail-log-result; health → hub-empty;
presence.get/set → presence-state; presence.request/changed/check_in → hub-empty;
all other methods → hub-mutation-receipt. Reply shape mismatch fails the operation.
Mutation receipts may be queued or terminal except pause/stop/step/manage.patch,
which wait for their specified boundary or time out; receipt polling recovers
their state. `hub.cancel` returns the original operation's cancelled receipt;
if already applied it returns `conflict` without changing that receipt.

## Presence and attention (2.1 only)

This section closes ADR-0036's messages. In ADR-0040's terminology the attention
is the attending router coglet; this extension does not implement router peering
or transfer of attending-router authority. In this single-router contract,
`cogent` is the existing `hub-id` of its configured attending router. It is a
scope selector, never proof of identity. Unknown/foreign scope MUST be refused.

| Method | Direction | Params | Success result |
|---|---|---|---|
| `presence.get` | authorized actor via runtime → router | `{cogent}` | `presence-state` |
| `presence.set` | authorized actor via runtime → router | `{cogent, base, add, remove, ?attention, ?limit, ?reason}` | `presence-state` |
| `presence.request` | authenticated hosted coglet via runtime → router | `{cogent, coglet, urgency, reason}` | `hub-empty` |
| `presence.changed` | router → subscribed runtimes / entering or leaving coglet | `presence-state` | `hub-empty` |
| `presence.check_in` | router → attention | `{cogent, coglet, status, held, dropped}` | `hub-empty` |

All use the existing closed request record with `target: null`, fresh durable
`correlation_id`, bounded `timeout_ms`, typed `params`, and mandatory
`context: {actor, ?ticket}`. Each stream has one terminal reply. Notifications
are requests with durable outbox IDs; retry byte-identically and deduplicate
before application, not new events on each retry. They are not an unbounded
subscription stream. Router runtime fanout routes to the intended recipients;
notification grants are scoped to the recipient and named cogent. A recipient
MUST NOT accept a device-originated changed/check_in as router authority.

`presence-state = {cogent, attention, present, limit, revision}`. `attention`
is a coglet-id; present/add/remove arrays contain at most 255 coglet-ids; limit
is 0..255, initially 4; revision is uint64, initially zero. Attention is implicitly
present, MUST NOT occur in present, and does not count toward limit. Present,
add and remove MUST each be unique. All referenced coglets MUST belong to this
cogent. Add/remove overlap and a resulting present count above the resulting
limit return `invalid_argument`. Removing a nonpresent member is a no-op.
Changing attention to an explicitly present member requires removing it in that
same change. Actual changes increment revision once; no-ops retain revision.
Revision overflow returns `resource_exhausted`, never wraps.

Set requires the exact `presence.set` grant under authenticated actor/ticket and
cogent scope. Supplying attention OR limit additionally requires the distinct
`presence.attention` grant, including unchanged values. Default provisioning gives
set to attention and owner, and attention/limit authority only to owner; sharing
the owner's runtime does not inherit it. Compare base to current revision and
commit the state, durable retry result and notification outbox atomically. Stale
base returns `conflict` with no side effect. Admission and application both
recheck current authority. Exact-ID identical retries recover their saved result;
conflicting ID reuse returns conflict, never a second mutation.

Presence never changes lifecycle or grants. Get and changed subscription need
separate exact grants. Entering/leaving coglets have implicit self-only changed
notification authority. Every runtime learns presence to forward local absent
messages to the router rather than bypass its holds. Stop/removal removes the
coglet from presence and delivers its hold in a final check-in. Attention changes
retain holds and deliver them to the new attention in its first check-ins.

A presence.request source is stamped by its runtime from its authenticated
hosted-source binding. The router MUST reject a params.coglet different from that
binding, even if the same runtime hosts it. Urgency is low/high, a hint only;
reason is nonempty text with at most 1024 encoded UTF-8 bytes (the JSON Schema
character bound alone is insufficient; implementations MUST check bytes too). A request does not itself change presence. Admit at
most one per coglet per 60000 router-monotonic milliseconds; earlier requests
return `resource_exhausted`. Equality at the interval boundary is legal.
An admitted request MUST be forwarded immediately to attention using the same
closed `presence.request` request shape on a router-originated stream (target
remains null; the receiving runtime dispatches it to its configured attention
coglet). The router asserts the original authenticated actor/ticket context,
retains the stamped source/urgency/reason, and uses a fresh durable notification
ID. Commit the rate timestamp, original request receipt and forwarding outbox
atomically before acknowledgement. Retry identical original requests from their
saved receipt without consuming a second rate slot or creating another forward;
the attention deduplicates the notification ID. Missing capacity returns
resource_exhausted without consuming the timestamp. A runtime accepts this
attention-facing notification only from its authenticated configured router;
it never re-originates it as a coglet attention request.

Only unsolicited messages from absent coglets addressed to attention are held.
Same-runtime sends take the same router path. Present traffic and traffic to
other destinations are unaffected. Answers AND follow-up updates carrying an
attention-originated call's correlation_id bypass holds only after matching the
retained authenticated call, sender and destination. An echoed ID or a message
kind such as ticket.update is NOT authority to bypass. Correlation history is
retained until the call/follow-ups end under the owning operation contract;
unknown or revoked provenance fails closed, never bypasses by guessing.

Each absent coglet's hold is FIFO with cap 64 by default (configuration may lower,
not raise, that cap). The 64th message triggers an early check-in when permitted;
early check-ins are at least 10000 router-monotonic milliseconds apart per coglet.
While limited, overflow drops the oldest message and increments dropped.
At an eligible early-check-in with an already full hold, drain the old hold first
and retain the incoming message for the next batch, never emit 65 entries.
`held` contains existing hub-inbox-delivery records oldest first. `status` is the
actual existing hub-status obtained with the router's explicit manage status
authority; no fabricated healthy value on lookup failure. If unavailable, retain
the batch and retry rather than draining it. Count drops since the last committed
check-in. Counter overflow refuses new admission with resource_exhausted.

Every frame still fits the 1048576 encoded-CBOR-byte limit, independently of
message count/Unicode character bounds. Before holding, refuse an individual
hub-inbox-delivery whose deterministic CBOR encoding exceeds 524288 bytes with
resource_exhausted and no hold/counter mutation. A check-in status encoding MUST
fit 16384 bytes; an oversized status is unavailable for this operation, retaining
the hold until a bounded status can be obtained. These bounds leave room for a
single held record plus status and fixed frame/context overhead; a large but
otherwise legal inbox event cannot become permanently undeliverable at the FIFO
head. Enforce an additional configured router
held-byte budget; refuse new admission if it cannot be retained, not unlimited
memory behind a count bound. Split check-ins at record boundaries for frame size;
retain unsent records, and report accumulated drops only in the first committed
batch. Check-in dequeue, dropped reset, retry bytes and destination outbox commit
atomically. A failed/unacknowledged send retries the retained batch, not a second
drain. Cadence is router configuration (a few minutes by default; slower when
empty), not a new remotely self-authorizing method.

Rate timestamps/counters use one durable router epoch across reconnect/restart.
Unprovable continuity fails closed (`unavailable`), never resets a rate bucket.
These temporal and authority constraints are separate from shape validation.
Spec-only oracles and golden vectors do not qualify a serving router or a real
client reply/expression stream. Product payloads remain product-owned.

## Lifecycle and boundary ordering

Install creates a stopped coglet with verified delegation, an empty private data
namespace and an initial verified staged code tree. Its `initial` field is the
exact staged revision hash; see the patch apply boundary below. No compiler runs
on the device. Identical durable retries return the original receipt; a different
install for an existing coglet is `conflict`. Initial patch staging cannot run code.
Start checks verified code, capability grants and kill switch; starting an
already running coglet is a no-op. Stop waits for a safe boundary, prevents the
next step and cancels pending actions using the adapter's cancellation rules;
it cannot retract physical effects. A hung step is interrupted by runtime
budgets. Paused is distinct from stopped. Start resumes paused operation.
Status returns state (`stopped`, `running`, `paused`, `faulted`), live tree and generation,
last committed step, pending mutation count and last shared error or null.

After daemon restart, before exposing device readiness, recover locally configured
installed coglets. Durable `running` intent resumes native scheduling only after
revalidating the installed signed artifact and recovered executable bytes,
installed identity, current delegation/capability grants, and local kill switch.
Automatic resume uses the original start's actor/ticket scope and pinned authority
generations, committed atomically with the running transition and bound to its
operation ID and intent digest; never infer authority from a receipt or reader.
Retained, locally configured and currently authorized generations must match
exactly. Rotation, revocation, rollback or missing/mismatched recovery context
requires a fresh authorized start, not adoption of newer authority on boot.
Resolve durably admitted boundary mutations in receipt order before resume;
pending stop/pause or an uncertain mutation outcome fences guest entry. Requests
not durably admitted create no recovery intent. Recheck authority expiry using
the device's trusted authority clock (not recovered logical time) and kill state
immediately before execution admission after validation. Failed/uncertain
admission never exposes `running`: with an intact readable/writable WAL and
established identity, durably report `faulted` with an error, including for
missing recovery context or latched kill. Only a fresh distinct start passing all
normal checks and atomically replacing recovery context may clear this recovery
fault; historical retries cannot. If WAL/identity integrity or durable fault
persistence cannot be established, fail startup without device readiness.
Stopped, paused and faulted coglets do not automatically resume. Recover committed
data, logical time and queued work under the ABI's
existing recovery/uncertain-attempt fences, without replaying an uncertain guest
attempt or duplicating committed effects. Recovery creates no new start receipt
and preserves historical receipt ordering; retrying an old start cannot reverse
a later stop. Here `running` means scheduler-enabled and eligible to execute when
input is ready, not continuous CPU use. See
[ADR-0027](../docs/adr/0027-daemon-restart-resume.md).

Boundary processing is serialized per coglet and durable: (1) finish or abort the
current step; (2) append audit and apply queued manage/inbox mutations in receipt
order; (3) verify/apply an admitted filesystem patch; (4) pin inputs and code for the
next step; (5) run at most one fresh Wasm instance; (6) journal intents before
dispatch. No writes, code changes or new inbox bytes become visible mid-step.
If the ABI refines journal encoding, it must preserve this ordering/provenance.
A failed mutation is audit-recorded, changes nothing, and does not block later
ones. Queue changes and their receipt/audit records commit atomically with data
so crash recovery cannot append an inbox message twice. Stopped/paused coglets
still process boundary mutations without executing guest code.

`manage.pause` is acknowledged `applied` only once no guest step is running;
it does not pause real-time adapter reflexes or the kill switch. `manage.step`
requires paused state, applies pending boundary work, executes exactly one nonempty
event batch, and stays paused, returning the committed step number. If no event
is queued, the runtime records a synthetic common `runtime.manage_step` event
(source = target coglet, next durable source sequence, current runtime logical
time, payload = `{}`, correlation_id = this step operation ID). This wake uses
the same source sequencer as other runtime-generated events, bypasses subscription
filtering and is journaled; no empty batch is passed to the ABI.
Concurrent step requests serialize; no implicit resume. On failure, remain
paused or faulted, never silently run. A step timeout can have committed; inspect
its receipt. `manage.read` uses one committed snapshot, never partially running
state. Its result includes step and data-tree hash. Reads/tails are bounded
ranges; polling is opt-in, no unsolicited private-state stream.

<a id="git-control-plane"></a>

## Filesystem patches and log tails

A coglet is a plain filesystem: `/code` is policy, `/data` is private state,
and `/logs` is append-only. There are no repository objects, refs or bundles.
`manage.patch` changes only `/code`; `manage.tail_log` observes only explicitly
granted logs. Each has its own exact-method grant; neither implies read, write,
pause, step or installation authority.

### Patch and FileChange

The closed `manage-patch` record is `{? base, revision, generation, files,
signature}`. `base` and `revision` are tree hashes (64 lowercase hex characters).
A code tree hash is the ABI 1.0 Git SHA-256 tree id applied to the snapshot
restricted to `/code`. The root holds a `code` tree entry, not just its subtree
id; empty code has the empty tree id (ADR-0035).
It is not the full ABI snapshot hash; `/data` and `/logs` are excluded. Empty
code trees use the Git SHA-256 empty tree id. The snapshot map remains only the
wire representation. Paths with a case-insensitive `.git` component and
file/directory collisions are rejected before hashing.

`files` contains 1..4096 whole-file changes, sorted strictly by UTF-8 path bytes,
with unique paths. A `FileChange` is `{path, ? bytes}`; `bytes` is **lowercase
even-length hex**, as required by the shared no-CBOR-byte-string profile.
Present `bytes: ""` puts an empty file; absence deletes (an absent file is a
no-op). Null is not absence. A file cannot also be an ancestor directory.
Paths are relative beneath `/code`: 1..32 ASCII segments, each 1..128 characters
from `[a-zA-Z0-9_.-]`, excluding `.` and `..` segments. No absolute path,
traversal, empty segment, backslash, control byte, symlink or non-regular-file
entry is allowed. Paths are case-sensitive bytes; never resolve through the host
filesystem or normalize case. An entry named `data` is still `/code/data`, never
`/data`. The private namespaces are inaccessible to patches.

`generation` is uint64 and MUST strictly exceed the durable watermark (initially
zero), scoped by pinned router and target coglet; never reset it on restart,
uninstall/reinstall or key rotation without explicit reprovisioning. At exhaustion
fail closed; rollback is a new higher generation, never a rewind. For an installed
target `base` MUST be present and equal the current code tree; for initial staging
it MUST be absent. Subsequent initial staging can replace a staged candidate only
with a higher generation. Staging's durable watermark fences replay too, but
status remains null until install. Stale base or non-increasing generation is
`conflict` (`details.reason`: `stale_base` or `generation`).

`signature` is lowercase 128-hex Ed25519 by the pinned router node key. Sign UTF-8
`coglet/hub/0:manage.patch\n` followed by deterministic CBOR of
`{hub, coglet, ? base, revision, generation, files}`: the Patch fields excluding
signature plus the pinned router node ID and request target. No signer supplied
by a caller can override that pin. Delegation still signs UTF-8
`coglet/hub/0:delegation\n` followed by its statement, excluding signer/signature;
its signer is the target coglet key, verified against the authenticated binding.
A signature never grants authority or binds an actor/ticket by itself.

A patch MUST leave valid `bin/step.wasm`, including the ABI exports/imports and
selected admission policy, and valid manifest/build provenance. Recompute the
candidate tree and require `revision` equality. Preserve `/data` and `/logs`.
Invalid path, revision, module or provenance is `invalid_argument`; a bad signer
or signature is `unauthenticated`; revoked authority is `permission_denied`.
Validate shape/encoding/limits, authority/signature, paths, base, generation,
resulting revision and executable/provenance in that order. No failure changes
live code, private data or generation. Bounded staging storage is released on
failed transfer or cancellation, not confused with durable admitted operations.

### Transfer, CAS and the apply boundary

`manage.patch` params are `{size, sha256}` for the complete core-deterministic
CBOR Patch, including its signature. `size` is 1..67108864 **encoded bytes**;
hex overhead counts toward this 64 MiB cap, not just file contents. Reject size
excess before allocation with `resource_exhausted`. Frame size stays 1 MiB.
Responder sends cumulative `hub-credit`; initiator sends contiguous `hub-chunk`
frames (max 32768 raw encoded-patch bytes each) from offset zero, then `hub-end`.
The raw concatenation MUST have exactly the declared length and SHA-256, decode
as `manage-patch`, and re-encode byte-identically. Missing/trailing chunks,
wrong offsets, digest mismatch or malformed bytes fail atomically. No partial
resume. Cumulative credit never decreases or exceeds size; unconsumed allowance
is at most 65536 bytes. Reject oversend/overflow; no credit by timeout is
`deadline_exceeded`. Credit/end/terminal frames are exempt from byte credit.

Admission durably records the original operation ID, canonical intent digest,
actor/ticket scope and pinned authority generations. Recheck authority at each
chunk and application. At application, signature, base, generation, candidate
revision, verified bytes, audit and terminal receipt MUST commit in one serialized
durable CAS. Status exposes only the applied live tree and generation. Concurrent
candidates cannot pass against an obsolete base. An exact operation retry returns
its retained receipt before fresh CAS checks; a different operation with the same
generation conflicts. Receipts never renew authority for a still-unapplied effect.

For installed targets, terminal `applied` occurs only at the serialized runtime
step boundary after the current attempt finishes/aborts and before input/code
pinning; replace the executable safely without running guest code during apply.
For an uninstalled target, ADR-0026's boundary is durable **verified staging**
consumed by install, together with the original operation's immutable completion
receipt and scope. No guest execution, installed private namespace or live tree
is created. A queued record or candidate's presence alone never proves completion.
Install's `initial` is the exact staged revision; require its applied receipt,
current authority, verified bytes and delegation, then atomically consume staging
and expose its generation. Locally trusted provisioning is a distinct source,
never a way to relabel queued remote work as applied. Crash recovery reconciles
the same original ID and receipt, never allocates a replacement operation or
repeats a confirmed effect. Installed hot-apply and pre-install staging are
separate implementation qualification gates.

### Log tail

`manage.tail_log` params are `{from_offset}`. It returns `manage-log` frames
`{type: "log", correlation_id, offset, hex}`, then one terminal success
`{next_offset}`. The offset is a byte position in a **per-coglet** durable,
ordered logical `/logs` append stream, not a directory enumeration, file-local
position, source event sequence or router-global counter. Every committed log
append contributes one canonical JSONL entry `{file_offset, hex, path}` in durable
append order. JSON bytes follow ABI inbox serialization (compact sorted keys,
unescaped Unicode, standard escapes, one LF). `path` MUST be a canonical absolute
`/logs/...` path in the full ABI UTF-8 domain, not the bounded ASCII subset
accepted by management read/write requests. Preserve spaces and exact Unicode
bytes without normalization or a separate log-entry path-length cap; forbid C0
controls, DEL, backslash, empty/dot/dot-dot components and trailing slash as in
the ABI. `file_offset` is that file's prior byte length; `hex` is nonempty
lowercase hex of the appended bytes, max 32768 raw bytes per entry (split larger
appends in order). These entries describe all committed log-file writes, including
stdout/stderr and structured log output; they are not extra guest-writable files.

Persist append bytes, entry order, logical cursor and highwater atomically with
the originating committed step/boundary. Expose only committed bytes; failed
attempts expose no entries. Restart or rotation never resets/reassigns an offset.
The logical stream is retained for the provisioning lifetime; storage pressure
refuses new appends before commit, never silently discards history. A tail cursor
survives process restart, independent of file rotation and request correlation ID.

The requester sends cumulative `hub-credit.bytes` relative to `from_offset`,
initially zero. The log owner sends contiguous chunks within credit, max 32768
raw logical-stream bytes each. A chunk may split a JSONL record or UTF-8 sequence;
concatenate before parsing. Each operation returns at most 65536 raw stream bytes;
credit cannot exceed that cap and unconsumed credit is at most 65536. Offset plus
length is checked for uint64 overflow. Recheck current actor/ticket and log/path
grants before every frame. A grant must cover every path represented by this
target's stream; otherwise refuse the whole tail, never filter bytes and corrupt
cursors. A narrower path grant may use bounded `manage.read` instead.

`from_offset` above highwater is `invalid_argument`; equality waits for new
committed bytes. End successfully at the 65536-byte cap or request timeout with
`next_offset` equal the first unsent byte, including empty successful timeout at
highwater. Credit starvation by timeout is `deadline_exceeded`, not permission
to buffer unbounded output. FIN/reset cancels this observation only; reconnect
with the first unconsumed byte. No mutation receipt or implicit subscription,
unsolicited stream, private-data export or batch publication is created.

### Build, rewrite, sign and replay

Build-then-patch is a router responsibility. Pin source and build recipe/toolchain,
compile core Wasm into a temporary **original** module, then apply the selected
publication policy before gates, signing or transfer. When the proposed
[ADR-0003](../docs/adr/0003-determinism-rules.md) policy is selected, run its
R1/R2/R4/R5 admission checks and R3 NaN-result rewrite on the router; validate the
rewritten module and run gates against those exact output bytes. Include the
output as `bin/step.wasm` together with source, manifest, prompts and the build
record below. Do not include the original under the executable path, run gates
only on the original, or rewrite on the device or during replay. Rewriting,
lint, validation or gate failure leaves staged and live code unchanged.
This defines the publication boundary, **not acceptance of ADR-0003** or a claim
that its transform/cross-architecture behavior has been qualified. Until policy
qualification, production admission under that policy fails closed.

`build/provenance.cbor` is a core-deterministic CBOR `hub-build-record`, included
in the signed resulting code tree. Its closed fields are:

- `version`: `"2.0"`;
- `source_tree`: code tree hash of the pinned source before build;
- `recipe_sha256`: SHA-256 of the retained build recipe/toolchain lock bytes;
- `policy_sha256`: SHA-256 of the retained policy document/parameters bytes;
- `transform_sha256`: SHA-256 of the retained rewrite tool artifact bytes;
- `original_sha256`: SHA-256 of the exact original compiled Wasm bytes;
- `rewritten_sha256`: SHA-256 of the exact `bin/step.wasm` bytes.

The build service retains those digest-addressed inputs and original/output bytes
for gate reproduction. Digests alone do not establish that a rewrite was correct:
the kernel MUST check the record against the pinned build inputs, qualified policy
and gate evidence for the rewritten digest before signing. If the selected policy
performs a no-op (for example a module with no affected float operations), both
module hashes may be equal; record the tool/policy anyway. Missing provenance or
an output hash mismatch is `invalid_argument` at code admission, with no activation.
Older transfer-only fixtures without this record do not establish code admission.

The router's patch signature authenticates the resulting code tree, including
the exact rewritten bytes and provenance binding both original and output hashes.
There is no signature on the original module that authorizes execution. A new
build requires a new record, gates and signed patch generation. Log tails require
no build record. Publication/history outside management is deferred to a Gitlet.

At activation, verify the signed patch and resulting code tree and record/output hash before pinning the
code at the boundary. Record the admitted signed patch and provenance in the
local admission audit; retain provenance at `/code/build/provenance.cbor` in ABI
snapshots. ABI `code_hash` and its retained `blobs` entry identify the **rewritten**
`bin/step.wasm`, not the original or a freshly compiled substitute. Replay loads
exactly that blob, verifies its SHA-256 and provenance output digest, and never
re-runs the compiler/transform or substitutes current code. Missing/mismatched
bytes fail replay closed. Promotion starts a new ABI trace with the new code;
rollback selects a previously admitted rewritten artifact with its provenance,
not a rebuild. Gate evidence must name that same rewritten digest.

Factory or builder output is never a promotion signature. Policy/gates and
probation remain kernel responsibilities, not bypasses granted by `manage.patch`.
Signing fixtures contain test-only keys, never deployment keys.

## Event and action relay

`hub.events` carries a bounded batch of unchanged common events observed for
its target coglet. Preserve source, seq, logical_time, correlation_id and payload
byte-for-byte; never inject provenance into a capability's closed payload. The
router records authenticated device, target and relay offset as admission metadata,
not as event fields. An adapter source is asserted by that enrolled device's local
event router, not independent proof of the adapter key and never cross-device authority.
A coglet source must equal target and may carry only a runtime-synthesized
`action.outcome` for an admitted router action. Foreign coglet sources and all
`runtime.*` events are rejected with `permission_denied`; local wakes never relay.
The router MUST enforce the target's event grants and capability ownership before
relay. Private speech events remain owner-only at the router: storage/routing MUST
NOT expose them to another coglet. Merely naming a target grants no access.

`params.offset` is a separate durable relay position per (device, target), starting
at zero. Assign contiguous positions when events enter the durable relay outbox.
The router atomically records batch digest, offset and resulting `next_offset` with
admission. New batches require offset equal to the stored next_offset; return
`next_offset = offset + event count`. Check uint64 overflow before admission.
An empty batch is a no-op cursor probe at the current offset and creates no
batch receipt or position reservation. Identical admitted
batch retries return their original result without rerouting; conflicting reuse
or a missing position returns `conflict` with `details.expected_offset`. Retain
batch receipts for the identity lifetime, independent of transport correlation IDs.
Event seq is NOT a relay position: for each (target, source), new admitted events
must increase strictly, but gaps are legal (subscriptions, private delivery and
local wakes consume source seq). Check batch retries before source watermarks.
Saturation returns `resource_exhausted` before admission; upstream retains reliable
events or explicitly records local loss before assigning relay positions.
No direct coglet-to-coglet connection, including siblings on the same device.
The router applies routing/policy and creates target inbox entries/events as needed,
subject to the capability's ownership restrictions.

`hub.actions` carries one common action. Its coglet must match target. Its deadline
is in the receiving runtime epoch, NOT the sending router epoch. The sender MUST
establish that mapping before first send and durably retain the mapped request;
no mapping means no new dispatch. The receiver preserves this intent and maps
the deadline once into the adapter epoch before first adapter dispatch, retaining
the complete act envelope. Retries/reconnects reuse the retained envelopes at
both boundaries, never a new sample/translation for an old ID (conventions,
ADR-0009 M-C). Runtime checks router authority, local capability and exclusive
output ownership and returns common action-accept in the result.
An immediate rejection is terminal, never an outcome. Admission is not completion;
one eventual common action-outcome is returned as an `action.outcome` event via
`hub.events` (correlation IDs match). Persist `(coglet,id)` envelope and admission
before dispatch. Conflicting reuse fails; identical retry does not re-dispatch.
On restart reconcile admitted actions with adapter receipts; when effects are
unknown, report failed with `unavailable` and explicit unknown-effect details,
never blindly replay. No exactly-once physical-effect guarantee. A kill switch
always wins. Expired actions cannot be dispatched even after reconnect.

## Inbox and manage mutation receipts

`hub.inbox` queues one delivery `{id, sender, event}`; event is the unchanged
common event envelope and `event.source` MUST equal sender. The persisted line
is `inbox-message` per [spec/abi.md (PR #8)](https://github.com/softmax-universe/coglet/pull/8),
not a second router-specific file format. The router's `hub-inbox-delivery` schema is
only an append request without the runtime-assigned index; it is never a persisted
line. Runtime assigns the next contiguous index and serializes the ABI line
(compact sorted-key JSON, unescaped Unicode and standard JSON escapes), journaling
the exact file JSON bytes,
appends UTF-8 JSON plus one LF at a boundary, and emits the ABI `runtime.inbox`
wake `{index,id}`. Index exhaustion fails closed. Newlines inside strings are
escaped; integers retain full precision. `sender` is authenticated router identity
(or the router-validated original source on relay); never trust an arbitrary sender
string. The router checks sender authority and the runtime trusts only the pinned
router's explicit provenance. LLM results/steering/factory notes use this primitive.
Runtime owns append; coglet owns its separate read cursor. Neither manage.write
nor guest code may truncate/rewrite the inbox. No embedded inbox import and no
implicit private export. Message `id` dedupes per target permanently, or until explicit
operator reprovisioning; same ID/different message is `conflict`.

`manage.write` replaces a bounded file at a canonical absolute `/data/...` path,
with UTF-8 text content and an expected pre-write ABI snapshot hash. No traversal,
empty components, C0 controls (U+0000–U+001F), DEL (U+007F), backslashes,
symlink resolution, inbox or runtime cursor/
receipt/audit internals. Permission/path limits and expected hash are checked
again at application. Failure is atomic `conflict`/`permission_denied`. There is
no arbitrary command execution or `/code` write via manage. Binary manage writes
are not in 2.0. Reads use similarly canonical `/data` or `/logs` paths (and
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
admission before side effects. Event dedupe retains source watermarks AND every
admitted nonempty batch's offset, digest and original next_offset for the device
identity lifetime. Even after later batches succeed, an identical older retry
returns its original success without re-delivery; changed bytes at that offset
return conflict. Check retained batch receipts before source watermarks. Empty
cursor probes reserve nothing; storage pressure refuses new admission, never
evicts an old receipt to admit work.
`not_found` on receipt means never admitted for that caller/target, not permission
to retry someone else's ID. An authorized caller can retry its original mutation
unchanged. Tail/status/read/health are observation operations and may be retried
with fresh correlation IDs; action IDs still retain their common meaning.

Each intervention audit record contains operation ID, actor, optional ticket,
method, target, applied boundary step, before/after data hashes and outcome.
Do not include secret file contents. Journal references the audit ID and marks
origin `hub` versus guest changes marked `coglet`; training export MUST preserve
that distinction. Audit is local, durable and append-only outside guest-writable
paths. If audit/receipt cannot commit, do not apply a write or inbox append.

## Health, limits and backpressure

Health is explicit metadata: device node ID, monotonic uptime milliseconds,
queue depth, available bytes, and per-coglet state/step/live tree and generation. No file
contents, prompts or transcript. Device reports every 10 seconds while connected;
a router marks degraded after 30 seconds without accepted health and disconnected
on transport loss. These are router-local elapsed durations, not clock comparison.
Health alone cannot authorize enrollment/start. Reconnection resumes no expired
grants, and queued operations are recovered from receipts.

At most 16 concurrent operation streams per peer, including at most 2 patch/tail
transfers, and 4 MiB of queued decoded request data per peer. Reserve one stream
slot for stop/cancel/health/receipt. Requests carry at most 64 events; individual
inbox/manage text is at most 65536 UTF-8 bytes. Queue capacity is also bounded per
target to 256 pending mutations; reject over capacity before admission. Reads
return at most 65536 bytes per call; a text slice must end on UTF-8 boundaries.
Wire/schema text character limits are coarse bounds; byte limits are additional
semantic checks. Router and device advertise no arbitrary unbounded windows.

QUIC credit is necessary but insufficient. Only patch/tail operations have
multiple data frames, with the explicit bounded credit rules above. Other
operations use bounded unary admission, never an unbounded hidden queue.
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
The legacy 0.1 `conformance/hub/` cases and runtime implementations are migrated
separately, not accepted as 2.0 coverage by these spec-only tests. The filesystem
patch, signed-negative and fresh-process tail vectors here are specification
oracles, not proof that a router/runtime or mock implements the new wire revision.
Snapshot hashes and persisted inbox/journal encoding are owned by [the ABI](abi.md):
the Git SHA-256 tree id of the snapshot including `/code`, `/data`, `/logs`,
excluding journal storage; see ABI 1.0 and ADR-0035. `manage.read.snapshot_hash`
and `manage.write.expected_hash` use that definition. No alternate hash or inbox
format is standardized here.

## Compatibility alias table (temporary)

Hub 2.0 peers MUST normalize the following method names before canonical schema
validation, grant lookup, operation-intent digesting, audit and receipt storage.
Provisioned grants use that same canonicalization; aliases confer no additional
authority. Reject an alias at or after **2027-02-25T00:00:00Z**, using trusted
authority time (fail closed if unavailable). No protocol downgrade is implied.
Canonical schemas, emitted events, journals and new clients use only new names.
Retained original signed delegation bytes MUST NOT be rewritten before signature
verification; normalize its method entries only afterwards for authorization.
Do not rewrite historical immutable journals or receipts during recovery.

| Accepted input before removal | Canonical name |
|---|---|
| `debug.read` | `manage.read` |
| `debug.write` | `manage.write` |
| `debug.pause` | `manage.pause` |
| `debug.step` | `manage.step` |
| `runtime.debug_step` (historical import only) | `runtime.manage_step` |

The removed Git control methods have **no aliases**, even during this window;
unknown/removed names fail `invalid_argument`. Runtime-generated wakes are never
accepted from a remote producer. The historical event mapping is an explicit
archive-import conversion into a new trace, never in-place hash mutation.
