# ADR-0046: Router peering and authority intersection

- Status: Proposed
- Date: 2026-11-28
- Authors: Coglet M0b specification owner
- Related PR: This ADR's accepting PR
- Supersedes / superseded by: Resolves ADR-0040's peering deferral only

## Context

[ADR-0040](0040-hubs-and-coglets-on-unforged.md) defers router wire changes.
The approved Baby Bear migration design (SHA-256
`aac083f0839a9f44eb9ab4f3bd821e91674c277725b60ec79c9555a75eb35f5d`,
§2, §4.6, §5 item 7) requires Softmax as attending router and the Mac mini as
home peer. Private stores stay home. Common conventions, router 2.0 and
[ADR-0038](0038-private-observation-ownership.md) remain authoritative.

## Decision

### Enrollment and topology

Select separate draft peering 0.1, ALPN `coglet/peering/0`. This changes neither
`coglet/hub/0` nor its frozen snapshots. Use common deterministic CBOR and router
framing: four-byte big-endian length, 1..1048576 body bytes, one request and one
terminal reply per bidirectional stream; malformed/trailing frames reset code 1.

Both local administrators independently pin the other's iroh key, household
scope (opaque correlation-id), attending hub-id and exact routes in durable local
policy. No TOFU, discovery, transitive trust or automatic failover. Each endpoint
sends `peer.enroll` with common version-offer and verifies the other's success
with common version-selection before routing. Only 0.1 is supported. Claimed
hub-id suffix MUST match authenticated transport identity; both bilateral pins
must already exist. Enrollment installs no grants. Reconnect renegotiates;
revocation fences existing streams. Pre-enrollment route races fail unauthenticated.
Softmax is attending, mini is home peer. Attending means attention routing, not
administrative authority or private audience. Attention transfer is not defined here.

### Routing and authority

`peer.route` carries household, sending/receiving routers, sender/target coglets,
actor and optional ticket, exact capability, receiver-logical deadline, original
correlation ID and profile-map payload. Both routers locally provision the exact
(household, routers, sender, target, capability) route and current delegation.
Receiver is the target's router: one hop only, no onward forwarding, wildcard,
service-to-service route or fallback destination. Sender authenticates origin
and actor/ticket through its local binding, never asserted strings; receiver
independently pins that sending router as attester for that exact origin binding.

Effective authority intersects **sending router policy, receiving router policy,
current delegation and executing runtime local adapter grants**, plus exact
route and actor/ticket permission. Each checks its own authoritative durable state
at admission, dequeue, execution and every disclosure, including after blocked
I/O. Neither router can widen, install or renew the other's local grants. Expired
tickets/delegation and unavailable authority fail closed. A wire field is never a
grant. Validate selected capability payloads, not just generic profile-map shape.

Private traffic additionally identifies source, owner coglet and source epoch.
Check claims against retained ADR-0038 provenance and trusted-local actor-to-owner
bindings at both routers and final runtime on every disclosure. Target must be
the retained owner. Missing, foreign, stale or fenced bindings deny. These claims
cannot transfer ownership. Receiver classifies data from retained provenance, not
requester flags: omitting private metadata never declassifies it. Deny mixed or
unclassifiable results. Replies, receipts, status, logs and error details share
this gate; unknown and inaccessible records receive indistinguishable bounded
refusals. No bulk store, raw audio or camera export. Owner-scoped answer export
and writing that answer to shared memory require separate explicit grants.

### Durability, outage and kill

Persist exact mapped request bytes before first send. Receiver atomically commits
inbox admission, metadata-only audit and receipt keyed by (sending router, sender
coglet, correlation ID), with request digest and original actor/ticket/target.
Identical bytes return retained admission; conflicting reuse conflicts. Accepted
means durable inbox admission, NOT adapter completion. Audit failure consumes no
ID. Retain text-free dedup tombstones permanently; finite capacity fails closed
instead of evicting identities. Check current disclosure authority on retained
receipts too. Bound frame, concurrency, pending plus completed observations and
retained bytes before admission. No exactly-once physical effect claim.

Map deadlines once to receiver runtime epoch before retention, per common rules;
missing mapping refuses, retries never remap or allocate a new ID. Lost replies
mean uncertain admission; reconcile the identical original request after mutual
re-enrollment, under current authority. Timeout does not cancel accepted work.
No automatic offline cross-router queue or reroute after partition recovery.

Softmax outage preserves independently granted mini-local low-risk direct calls
under the product's pinned risk catalog; all cross-router intents fail closed.
A client direct call uses ordinary authenticated runtime enrollment to mini,
not authority inherited from Softmax. Until qualified, only mini-local room-voice
calls exist. Local grants do not depend on server liveness. No automatic attending
election. Local latched kill fences only that runtime's adapters/grants: server
kill does not fence mini and mini kill does not fence server. House-wide stop is
a distinct operator action at each local kill boundary, not a remote kill claim
across a partition. Reset is local and requires fresh grants.

## Alternatives

- Frozen hub extension: conflates runtime enrollment and bilateral attestation.
- Universal attending-router trust: widens receiver grants and private audiences.
- Offline queue or automatic election: creates delayed effects and split authority.

## Consequences

ADR and schema authorization only, no serving implementation. Classify `peering`
as `draft-validated`, preserving generated hub/types. Integration needs separately
authorized real transport/durable authority qualification. This selects neither
Git-user transport nor ownership transfer. Current interim single-router topology
remains legal; no draft payload is legal on frozen hub frames.

## Verification

Retain 13 base `test_hub.py`, 9 `test_schema_lint.py` and 7
`test_contract_bindings.py` tests; add peering dual-schema golden/root/selected
negative cases and a spec-only authority oracle for unauthenticated routing,
authority widening and missing cross-owner bindings, with authorized controls.
Run full spec discovery, codegen freshness and
`cargo test -p coglet-types -p coglet-proto`. These prove contract shape and oracle
behavior, not authenticated execution. Unexecuted successor obligations: real
mutual iroh enrollment/revocation; blocked-stream epoch fencing; independent-Store
receipt/audit rollback; partitioned local-call positive; local-kill isolation on
both runtimes. Qualification cannot be inferred from this spec PR.

## Approval

Pending independent exact-head review, required CI and merge-queue landing.
