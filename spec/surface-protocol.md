# Surface protocol — 0.1

Normative vocabulary, identity grammar, versions, clocks, errors and event/action
envelopes are imported unchanged from [conventions](conventions.md).
Decision: [ADR-0007](../docs/adr/0007-surface-speech.md), [PR #9](https://github.com/softmax-universe/coglet/pull/9).
Schemas: `messages/surface.cddl`, `messages/surface.schema.json`. Each message
is a closed `{type, body}` record; table names select the body definition.

## Binding and negotiation

The surface supervises the runtime, owns native I/O and the off-switch. The
runtime is the only peer allowed to dispatch for its enrolled coglets. A local
socket MUST be private to the surface's user (directory 0700, socket 0600), verify
OS peer credentials against the configured runtime UID/process supervisor, and
use a supervisor-provided binding of surface key and permitted coglet keys.
There are no credentials in hello. A claimed identity is not authentication.
Same-UID hostile code is outside this isolation boundary; use OS sandboxing
where needed. A C ABI caller is trusted application code with the same configured
binding, not a dynamically loaded coglet. Unbound peers get `unauthenticated`
and no capabilities, grants or device data. Never expose this socket on a network.

Surface initiates `hello {surface, offer, clock, correlation_id}`. `offer` is the
common version-offer; clock is its logical microsecond reading at serialization.
Runtime returns `hello_reply {selection, correlation_id}` using common selection
rules. On failure it returns `error {error, correlation_id}` and closes. Only 0.1
is supported here. No operational message precedes a successful reply.
Surface then sends `capabilities {revision, clock, entries, correlation_id}`;
revision starts at 1 and increases on every full replacement. Entries contain
`name`, `events`, `reads`, `actions`, `exclusive` (resource names), and `schemas`
(a map from each advertised event/read/action name to a pinned schema URI).
Names are common capability-names; entry names are unique. A capability's
operations MUST appear in exactly one entry, and every operation needs a schema.
Empty lists are allowed. Schema URIs describe data, not executable URLs; never
fetch untrusted schemas during dispatch. Unknown capabilities may be ignored,
never assumed available. Capability removal revokes affected grants first.
Clock samples map runtime deadlines into the surface epoch; elapsed time is
advanced on the surface's monotonic clock. Reconnection invalidates that mapping;
no action is sent until a fresh sample has been established. Clock rollback is a
fatal protocol error. Cross-source event clocks are not compared numerically.

## Message directions

| type | Direction | body / semantics |
|---|---|---|
| hello, capabilities | surface → runtime | as above |
| hello_reply | runtime → surface | as above |
| grants | surface → runtime | full authoritative grant snapshot |
| event | surface → runtime | common event, source bound to this surface |
| read | runtime → surface | `{coglet, capability, args, grant_epoch, deadline, correlation_id}` |
| read_reply | surface → runtime | `{correlation_id, ok:true, value, snapshot}` or `{correlation_id, ok:false, error}` |
| act | runtime → surface | `{grant_epoch, action}` with common action |
| accept | surface → runtime | common action-accept |
| health | either | `{state, clock, pending, correlation_id}` |
| kill | surface → runtime | `{reason, correlation_id}` |
| error | either | `{error, correlation_id}`; not a substitute for action outcome |

`event kind=action.outcome` carries the common action-outcome as payload.
Outcomes are routed using the persisted action's coglet, never broadcast.
Read is side-effect-free, returns a single bounded snapshot with a uint64
`snapshot` revision, and checks grants and exclusive deadline at admission.
Each read gets one reply; timeout locally reports `deadline_exceeded` and late
replies are discarded. Unknown correlation IDs are protocol errors. Reads can
be shared, but are delivered only to their requesting coglet. Event subscriptions
are intersected with each coglet's grants; the runtime fans out authorized events
without permitting direct coglet-to-coglet communication. Private speech events
are unicast to their resource/session owner, not a shared device
broadcast. In the speech profile, active duplex input/output requires the same
coglet to own both audio.mic and audio.speaker, preventing cross-owner barge-in
metadata leaks. Configured schema URIs resolve only through the supervisor's
release-pinned trusted schema catalog (not mutable network lookup).

## Grants and fencing

`grants {epoch, entries, correlation_id}` replaces the full snapshot, not a delta.
Each entry is `{coglet, operations, resources, expires}`: operations are allowed
names; resources are owned exclusive resource names such as `audio.mic`,
`audio.speaker`, `display.face`. Expiry uses the surface clock (exclusive bound).
Epoch starts at 1, strictly increases for each replacement and persists across
restarts. All requests MUST carry the current epoch and the action/read coglet
MUST be present, unexpired and allowed that operation. Both runtime and surface
check; the surface is final authority. A mic lease fences capture/private input
as well as output. A speaker lease fences speech, backchannels and provider audio.
A resource has at most one coglet owner in a snapshot. Nonexclusive reads can be
shared. Manifests needing missing capabilities or exclusive resources cannot run.
Hub policy selects owners, but cannot exceed local OS/user permission or clear
kill. The trusted surface supervisor installs authenticated hub-policy decisions
through its local configuration API; that product-specific provisioning path is
not a surface wire message. It intersects policy with local permissions before
publishing effective grants. Runtime cannot manufacture grants by sending this
message in reverse. Capability removal publishes the revoking grants epoch
before the higher capabilities revision.

Revocation/expiry stops affected native I/O, cancels admitted work, removes private
queued events, and increments epoch before reassignment. Old-epoch commands for NEW action IDs fail
`permission_denied`, even if the same coglet later regains ownership. Exact
retries of a durably recorded action return historical receipts only (see below),
never re-admit work or acquire current resources. Delayed
provider callbacks are fenced by both epoch and session ID. A transfer MUST NOT
allow old audio to drain under the new owner. Invalid grant snapshots (duplicate
owners, expired entries, unknown operations) are rejected atomically with `error`.

## Admission, recovery and bounds

Accept is admission, not playback completion. Validate identity, epoch, operation,
resources, arguments, deadline and bounded queue before returning accept=true.
Rejected actions have no outcome. Every accepted action gets exactly one terminal
outcome event; duration/deadline overrun expires it and stops its physical effect.
Rejections use common errors (`conflict` for conflicting ID reuse, not an override).
Runtime journals intent before sending. Exact recorded retries are resolved before current-epoch
admission checks, but only for the originally authenticated/bound coglet; they
cannot execute an effect or authorize a different owner. Surface durably records full envelope,
coglet, epoch, rejections, admission and terminal result before acknowledging; recovery from
crash stops uncertain physical work and records failed/unavailable, never replays
an uncertain effect. This provides deduplication, NOT exactly-once physical effects.

Retain deduplication records until at least 24 hours after the later of terminal
time and deadline. Bound maximum accepted deadline to 60 seconds ahead; reject
farther deadlines with invalid_argument. On retry within retention, an identical
(coglet,id) envelope/epoch returns the stored accept and, if terminal, the stored
outcome (identical outcome payload, but a fresh source seq on each delivery);
runtime deduplicates terminal results by (coglet,id), not by event seq.
An accepted retry while pending must not execute twice. After receiving a
terminal rejection (including stale epoch), the runtime may issue a NEW action ID
and current epoch for the desired effect; it MUST NOT change IDs while admission
is unknown. Identical retries recover the original receipt first. Rejected reads
may be resubmitted with a fresh correlation ID and epoch because they have no
effects. A changed envelope or
epoch for the same key is conflict. After retention callers MUST NOT retry an
old ID. On lost ledger, fail closed and require reconciliation plus a new surface
identity before accepting new work; never pretend unknown action completion.
Recovery status is observed via replay of identical requests; no new recovery RPC.

Limit each frame to 1,048,576 bytes and each direction to 128 queued control
messages by default (implementations MAY lower advertised application budgets,
not the wire limit). Overflow MUST NOT silently drop transcript finals/outcomes:
stop input, fail closed with resource_exhausted and disconnect if it cannot be
reported. Native kill/barge-in bypass these queues. Connection loss immediately
stops output/capture, cancels pending actions into the ledger, clears grants and
requires new negotiation and a higher grant epoch. Event seq persists and
increases on every delivery, including repeated historical action receipts.

## Health and kill

Health state is `ready`, `degraded` or `stopped`; `pending` counts admitted actions.
Both sides send health at least every 1 second while connected. Three seconds
without a valid message/health triggers the disconnect procedure. Health is not
a grant, nor does `ready` restart stopped devices. Surface kill is local and
latched: immediately stop mic, speaker, display effects and provider sessions,
revoke all grants, cancel accepted work, then best-effort send kill and stopped
health. Kill works with a hung runtime or full queue. A kill message is a
notification of an already-applied reflex, not a request needing an ack. No
message/coglet/hub operation clears it. A still-open link rejects new action IDs
and reads with permission_denied; previously accepted work is cancelled. The
surface MAY close after best-effort notification, so clients must also handle
disconnect rather than wait for rejection. Only explicit local operator reset and
fresh connection, negotiation and grants may resume. OS suspension on iOS uses
the same stop/revoke path; hub fallback requires a separate authenticated session
and explicit new grants, not a live local lease duplicated onto another host.

## Transports

### Local stream socket

Use an AF_UNIX byte stream (not legacy SP/1 or a new network protocol). Each frame
is a four-byte unsigned network-order length N followed by exactly N bytes of
one deterministic CBOR message. N=0 or N>1,048,576 closes the connection before
allocation. Fragmented headers/payloads and multiple frames per read are normal;
EOF mid-frame is fatal. Reject non-profile CBOR, trailing data, unknown fields,
unknown message type, wrong direction and invalid lifecycle before side effects.
Socket write success is not accept. Audio frames never use this control channel.

### Embedded runtime C ABI (iOS)

The following names/signatures define the surface-message boundary of runtime
library ABI v1, not the guest Wasm ABI:

```c
typedef struct coglet_surface_link coglet_surface_link;
int32_t coglet_surface_open(uint32_t abi_version, coglet_surface_link **out);
int32_t coglet_surface_send(coglet_surface_link *, const uint8_t *cbor, size_t n);
int32_t coglet_surface_recv(coglet_surface_link *, uint8_t *out, size_t cap, size_t *n);
void coglet_surface_close(coglet_surface_link *);
```

`open(1,...)` creates a stopped/unnegotiated channel in an already-configured
runtime library; other ABI versions return -1 without creating a handle. No
identity is granted by open. Configure trusted application bindings before open.
`send` copies one surface→runtime CBOR item (no length header), validating the same
size, profile, schema and state machine as socket. `recv` polls one
runtime→surface item. Return codes: 0 success, 1 would-block/no data, 2 output
buffer too small (sets required n, does not dequeue), -1 invalid argument/ABI,
-2 closed/fatal protocol error. Would-block send copies nothing; retry the same
bytes. On recv=1 set n=0. Buffers are caller-owned; library retains no pointers,
returns no pointers into its memory, and never calls into surface code. Null
pointers are invalid except recv out=NULL/cap=0 for size query (return 2 if data).
Operations on a handle are serialized by the caller on a non-audio control queue;
no reentrancy, no calls after close; close frees handle and applies disconnect.
An independent native off-switch never waits on this queue. Both transports use
identical deterministic CBOR bodies and limits; frame-prefix bytes are the only
socket-specific bytes. iOS uses WAMR interpreter, never JIT. Suspending the host
closes this channel, not a promise of background execution.

Native adapter conformance MUST additionally probe unsupported open(2), a
recv(NULL,0) size query, short-buffer retry without dequeue, exact-buffer read
followed by would-block/n=0, would-block send with no copy/effect, malformed
send without dispatch, and close with pending native I/O. Wire scripts alone
cannot prove these pointer/ownership semantics.

## Verification

`spec/tests/test_surface_speech.py` validates schemas/goldens, socket framing
vectors and scripted temporal case expectations. `conformance/surface/*.json`
supplies executable transport-independent stimuli/expected results for the
black-box harness. Fragmentation vectors live in `spec/fixtures/surface/framing.json`;
the native ABI buffer probes above require a native adapter test, not these wire
scripts. These are inputs for testing, not an implemented runtime conformance claim.
