# Presence — router 1.1

Decision: [ADR-0036](../docs/adr/0036-distributed-cogent-and-presence.md),
[PR #182](https://github.com/softmax-universe/coglet/pull/182).
This is an additive extension to [router 1.0](hub-protocol.md), not an extension
to the private, implementation-only `coglet-types-hub0` snapshot. Normative
keywords and common types are imported from [conventions](conventions.md).
Shapes are generated into `messages/hub.cddl` and `messages/hub.schema.json`.
Shape validation alone is not authorization, temporal validation or conformance.

## Negotiation and messages

Router 1.1 uses the unchanged `coglet/hub/0` transport framing and explicitly
negotiates `1.1`. A peer selecting `1.0` MUST NOT send or receive presence methods,
presence results or presence delegation grants. The preserved `hub-v1-request`
root rejects presence operations; the session validator additionally checks the
delegation method list. All existing 1.0 messages keep their meanings and bounds.
`presence.attention` is a grant name, never an invocable RPC.

`CogentId` is `cogent:<node-id>`. Provisioning binds that identity, owner actor,
router and member coglets; a supplied ID is not enrollment or proof of ownership.
Exactly one configured attention coglet is always implicitly present, excluded
from `present` and `limit`. The limit is 0..255, default 4. Lists are duplicate-free.
Attention and all listed coglets MUST be members of this cogent. `revision` begins
at zero and increases once for every effective atomic state/cadence change.
Overflow fails `resource_exhausted` without mutation. No-op removal does not
advance revision or emit a changed event.

All requests use the existing request envelope, timeout and authenticated
`context: {actor, ? ticket}`; no context supplied by a guest is trusted. The
router binds context to its authenticated caller/delegation before grant lookup.

| Method | Direction; target | Params | Success result |
|---|---|---|---|
| `presence.get` | caller → router; null | `{cogent}` | `presence` |
| `presence.set` | caller → router; null | `{cogent, change, ? cadences}` | `presence` |
| `presence.request` | hosting runtime → router; null | `{cogent, request}` | `hub-empty` |
| `presence.changed` | router → runtime; recipient coglet | `{presence}` | `hub-empty` |
| `presence.check_in` | router → runtime; attention coglet | `{cogent, revision, check_in}` | `hub-empty` |

`presence` is `{cogent, attention, present, limit, revision}`.
`change` is `{base, add, remove, ? attention, ? limit, ? reason}`. Check grant
before disclosing current state or testing CAS. Stale `base` gives `conflict`.
Overlap, duplicate entries, foreign members, explicitly listing attention, or a
result exceeding limit gives `invalid_argument`, with no partial change. Optional
`cadences` contains unique `{coglet, interval_ms, idle_interval_ms}` entries for
members; both durations are 10000..86400000 milliseconds and idle MUST be at least
interval. Default active cadence is 180000 ms, idle cadence 900000 ms. These are
router-monotonic durations, not cross-runtime timestamps. Cadences are changed
by the same `presence.set` grant and revision CAS. Reasons are nonempty text,
with both a 1024-character shape cap and 1024 UTF-8-byte semantic cap.

`request` is `{coglet, urgency: "low" | "high", reason}`. The runtime fills
`coglet` from the originating installed coglet, not guest data. The router checks
its current relay binding and membership. Delivery to attention is immediate,
independent of the source's presence. At most one accepted request per source
coglet per 60000 ms, including across attention changes/reconnect; rejection is
`resource_exhausted`, with no delivery or reset of the accepted timestamp.

## Authority and notifications

Every method requires its exact-method grant scoped to cogent/recipient and actor,
with ticket provenance and revocation rules unchanged. Provisioning defaults
`presence.set` to the configured attention actor and owner actor, and
`presence.attention` only to the owner. A carrying peer is not that actor and
MUST NOT lend its owner's authority to other coglets on the peer. Supplying
`attention` or `limit`, even unchanged, additionally requires `presence.attention`.
Changing attention is not a grant provisioning RPC: any authority for the new
attention MUST already exist under separately configured policy. Presence MUST
NOT create, widen, transfer or revoke method, capability, ticket or path grants.
It MUST NOT call start, stop, pause, install or any other lifecycle operation.
The tests `presence_changes_no_grants` and
`presence_never_starts_or_stops_coglets` are required negative controls.

A separately granted `presence.changed` subscription is provisioning state, not
an implicit subscription from `get`. Each effective change emits the new snapshot
to subscribers and to every entering/leaving coglet, including old/new attention.
Each coglet is implicitly authorized to receive its own entering/leaving snapshot,
not other members' private messages. A runtime learns the cogent snapshot over
its authenticated router connection before enabling presence-aware local routes.
Lower revisions are ignored; equal revisions must be byte-identical or fail closed.
No 1.1 state is sent over a 1.0 link; a cogent with an old or unsynchronized member
runtime cannot enable presence-aware local delivery on that runtime.

## Routing, holds and call provenance

Only unsolicited member-to-attention messages are held when their source is
absent. Other destination routes are unchanged. Present sources and attention
itself deliver immediately. A runtime which would deliver locally to attention
MUST forward the presence-controlled message to the router; it MUST NOT create a
second local hold or deliver a duplicate. Missing/stale presence synchronization
fails closed for this route, not as implicit presence.

A caller-chosen correlation string is not proof of a call. The router MUST retain
an authenticated call ledger keyed by `(cogent, attention, callee, correlation_id)`
when the attention initiates a call. Only messages from that bound callee while
the call is outstanding can bypass holds as answers/follow-up updates. Retirement
and timeout are explicit; reconnect and reusing a correlation ID do not create
new authority. Unbound or guessed IDs are unsolicited. Retain bound updates,
including `ticket.update`, for the lifetime of the call, not just until its first
answer. Lifecycle, call-ledger and relay revocation are checked before forwarding.

A held Message uses the existing `hub-inbox-delivery` shape; `sender` MUST equal
its event source, the bound member coglet. Message IDs deduplicate durable retries;
conflicting reuse is `conflict`. Per-source holds contain at most 64 messages and
at most 524288 deterministic-CBOR bytes in total. Reject an individual Message
larger than that with `resource_exhausted` before consuming its offset or ID.
When accepting a message fills either bound, schedule an early check-in, at most
once per source every 10000 ms. While early delivery is rate-limited, drop oldest
messages until both bounds hold; increment `dropped` per removed message. The
counter MUST NOT wrap: exhaustion rejects admission without consuming the message.
Account source order durably, independently of wall-clock and transport streams.

`check_in` is `{coglet, status, held, dropped}`, with `status` the existing
`hub-status`, `held` oldest-first and `dropped` since the previous acknowledged
check-in. The router reads status through the manage port's status operation
(current wire name `hub.status`), using its own explicitly provisioned status
grant, never the attention's grants. Failed status reads do not fabricate a
status, expose cached private state or consume the hold; retry under normal
bounds. Idle sources receive status-only check-ins at the slower cadence.

## Atomicity, lifecycle observation and recovery

State changes, revision, changed notifications and pending check-ins MUST commit
atomically before acknowledging a set. Durable request ID plus authenticated actor/
ticket scope stores the terminal result; identical retries return it, conflicting
reuse fails. CAS is not sufficient to recover an acknowledged but lost reply.
Revision and operation IDs survive reconnect/restart. On promotion to present,
flush an existing hold first in an immediate check-in, then deliver fresh traffic.
On attention change, retain every hold and schedule its first check-in to the new
attention. Never send retained private data to the old attention after that fence.

The router observes independently authorized stops/removals: remove that coglet
from present, increment revision if effective, notify recipients, and send its
hold in a final check-in. This observation MUST NOT itself stop anything. A stopped
attention fails delivery closed until an authorized attention change; it does not
start a replacement. Never infer removal from a transient transport disconnect.

Check-ins have durable router-generated correlation/delivery IDs. Keep the exact
snapshot, dropped count and ordered held IDs until the authenticated receiver
acknowledges durable deduplicated admission. Retrying after timeout/restart uses
the same ID and bytes; acknowledge/removal is atomic. New arrivals use a new batch
and cannot be erased by the old acknowledgement. Reassignment fences unacknowledged
old-attention delivery and reissues it to the new attention, with receiver-side
admission checking current revision. Finite non-evicting dedup retention exhaustion
fails closed, never grants exactly-once by dropping history. Holds and call state
are private router data, absent from ordinary health/error/audit exports.

## Qualification

ADR-0036's sixteen named wire cases plus
`presence_never_starts_or_stops_coglets` MUST execute against both real router and
mock. Include guessed-call-ID, cross-actor/cross-cogent, lower-version, byte-bound,
restart/lost-ack, status-grant revocation and local-forwarding controls. Schema
fixtures and spec-only oracles do not qualify runtime transport or the router.
