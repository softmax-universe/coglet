# ADR-0045: Presence and attention router wire

- Status: Proposed
- Date: 2026-11-28
- Authors: Coglet contributors
- Related PR: pending
- Supersedes / superseded by: refines ADR-0036; does not specify router peering (ADR-0040)

## Context

ADR-0036 defines presence but no closed router frames. The approved Baby Bear
migration design §4.2 and §5 item 6 requires the wire contract before the turn
and reply clients are implemented. This card authorizes specs, fixtures and
SDK generation, not a serving router implementation or product message catalog.
The source design has SHA-256
`aac083f0839a9f44eb9ab4f3bd821e91674c277725b60ec79c9555a75eb35f5d`.

## Decision

Introduce additive router version **2.1**, retaining the exact 2.0 methods and
meanings. Presence frames MUST NOT be emitted on a 2.0 session. Existing frozen
snapshots and serving implementations pinned to them remain unchanged; this
contract is not a claim that those implementations serve presence.

Add closed requests `presence.get`, `presence.set`, `presence.request`,
`presence.changed` and `presence.check_in`. Each uses the existing request and
reply framing, correlation identifiers, timeouts, common errors and explicit
actor/ticket context. The target is the attention-independent router scope
(null); each request names its cogent using an existing hub-id (the attending
router identity, not a new authentication credential). Peering and transfer of
attending-router ownership remain excluded. Notifications are router-originated
requests acknowledged with the empty result. Get/set return a Presence snapshot;
request returns an empty acknowledgement, not acceptance into presence.

Presence contains cogent, attention, present, limit and revision. Limit is 0..255,
default 4; attention is implicitly present and excluded from the present list
and its cap. Set carries base, add, remove and optional attention, limit, reason.
All referenced coglets must belong to the named cogent; arrays are unique,
add/remove disjoint, and the resulting present set must fit the resulting cap.
Removing a nonmember is a no-op. Actual changes increment revision once; no-op
sets retain it. Overflow fails closed. The base comparison and mutation are one
durable atomic operation. Stale base returns conflict without a mutation.

Authority is scoped to authenticated actor/ticket, method and cogent, intersected
with the runtime relay binding. Set needs its exact grant; the presence.attention
grant is additionally required whenever either attention or limit is supplied,
even if its value is unchanged. Owner and attention defaults are provisioning
policy, never inferred from transport peer or a claimed actor. Runtime stamps
presence.request's coglet from its authenticated hosted source, never from guest
payload alone. Changed notifications are delivered to authorized subscribers,
runtimes, and entering/leaving coglets (self-only implicit notification grant).

Only absent-coglet unsolicited messages addressed to attention are held, including
same-runtime messages forwarded to the router. A matching correlation identifier
alone is insufficient for bypass: the router must retain an authenticated call
from the attention to that sender and authorize its answer/follow-up. Authorized
answers and follow-up updates bypass holds; other destinations are unaffected.
This generic routing applies to product reply/expression payloads without defining
Baby Bear payloads or granting them new authority.

Each absent sender has a 64-message hold by default, bounded also by the existing
encoded frame and router byte budgets. At capacity trigger an early check-in,
at most once per sender per 10000 router-monotonic milliseconds. While limited,
drop oldest messages and increment the dropped counter atomically. Check-in
contains coglet, actual hub-status, held inbox-delivery records oldest first and
dropped since the last committed check-in. At most 64 held records per frame;
configured caps above 64 are not introduced here. Delivery, counter reset and
outbox retention must commit together; retained notifications are retried with
identical IDs and bytes, not drained again. Counter overflow refuses admission.
Attention-request admission is at most one per coglet per 60000 router-monotonic
milliseconds; earlier requests return resource_exhausted with no state change.
Clock continuity and rate counters survive reconnect/restart; unprovable clock
continuity fails closed. Exact interval boundaries are admitted.

Attention changes send held messages to the new attention in its first check-ins;
stopped/removed coglets leave presence and get a final check-in. Cadence remains
router configuration (a few minutes by default, slower for empty holds), not an
additional self-authorizing method. Presence changes never alter grants or
lifecycle. Full notification byte limits may require smaller check-in batches;
never silently drop excess bytes or claim an oversized frame was delivered.

## Alternatives

- Reuse open inbox payloads: lacks typed closed methods and explicit authority.
- Extend 2.0 silently: violates minor-version emission gating.
- Hold all correlated traffic or bypass on any echoed ID: respectively stalls
  the attention's work or lets an untrusted sender evade holding.
- Add peering or product catalogs here: separate authorized migration work.

## Consequences

Schemas validate shapes; grants, CAS, set membership, correlation provenance,
rate limits and drop accounting require separate executable semantic oracles.
Generated SDK types are not serving-router qualification. Native transport and
real product turn-stream qualification remain subsequent implementation work.
Existing frozen contracts are not rewritten. ADR-0036's spec-with-implementation
schedule is refined for this explicitly authorized contract-first M0 milestone.

## Verification

Run the pre-existing hub, schema-lint and contract-binding regression suites at
cf6039de9c before and after. Add dual-validator golden vectors and recursive
unknown-field negatives; public hub roots as well as selected rules must reject
malformed frames. Add spec-only temporal cases for attention grants, stale CAS,
cap/overlap/foreign membership, hold overflow, counted oldest drops, exact 10 s
and 60 s boundaries, authenticated correlated answers/follow-ups, and attention
handoff. Run codegen freshness, generated Rust/Python/Swift codec coverage and
`cargo test --workspace --exclude coglet-engine-wamr`. Serving-router conformance
is not claimed by these spec oracles.

## Approval

Pending independent exact-head review and accepting PR.
