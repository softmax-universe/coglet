# ADR-0004: Hub control and relay protocol

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet Hub
- Related PR: recorded in the task and GitHub PR discussion
- Supersedes / superseded by: none

## Context

The design of record needs an implementable hub boundary before runtime and hub
implementations begin. Common conventions define identities and envelopes, but
not authenticated device delegation, git transfer, debug mutation ordering or
backpressure. Baby Bear must use this protocol beside, not inside, its RPC ALPN.

## Decision

Specify `spec/hub-protocol.md` at version 0.1 on `coglet/hub/0`, one bidirectional
QUIC stream per operation. Import common deterministic CBOR and envelopes.
Use one-use, expiring enrollment invites bound to authenticated iroh keys;
explicit coglet-key-signed delegation binds a hosted coglet to its device node.
Authorize exact methods and targets; debug read never implies debug write.

Use framed bounded operations and application credit, durable mutation receipts,
and reconnect recovery without promising exactly-once physical effects. Queue
inbox and debug mutations at step boundaries and audit their provenance.

Transfer bounded git bundles as hex strings in profile CBOR. Hub code updates
are build-then-commit, hub-key signed and compare-and-swap; the coglet signs its
published ref attestation with its own node key. Logs batch separately; private
`/data` never enters git. Define signing domains and anti-replay generations.

## Review revisions

Require actor/ticket context on every hub-originated observation (including
status, private debug reads and git exports), not only interventions. Local
authorization uses the exact actor/ticket scope and rechecks before disclosure.
Ticket absence cannot inherit ticket grants.

Distinguish authoritative ref CAS from observer catch-up: a fully verified signed
snapshot with a newer generation may skip intermediate observations. Equal but
different statements and older generations conflict; persist observer watermarks
atomically with the tracked ref. Requiring the observer's old tip to equal the
publisher's previous tip would permanently strand disconnected observers.

Publication clarification: integrate ADR-0003's proposed rewrite at hub build time,
before gates and the signed artifact commit. A closed `hub-build-record` in the
signed archive binds source, recipe, policy, tool, original and rewritten hashes.
The executed/replayed hash is the rewritten module; runtime/replay never rewrite.
This does not change or accept ADR-0003's transform policy or its qualification.
Fixtures cover record binding and declare future post-run publication/replay
oracles explicitly; a schema pass is not transform or engine qualification.

## Alternatives

- Multiple ALPNs duplicate authentication and negotiation; reject.
- Reusing Baby Bear RPC couples a product to the framework; reject.
- Trusting coglet ID strings on device connections permits impersonation; reject.
- Raw bytes require changing the common CBOR profile; hex chunks avoid that change.
- Automatic git merging or debug writes mid-step destroy deterministic replay.

## Consequences

No compatibility with prior protocols is implied. Enrollment secrets are not
logged. Device loss requires explicit delegation revocation/replacement. Durable
journals and bounded deduplication consume storage; exhaustion fails closed.
Debug and log access remains opt-in. Publication keys are isolated per coglet,
not the hub promotion key. Runtime implementations must verify signed statements
and imported objects before applying refs.

## Verification

`spec/tests/test_hub.py` will validate hub golden positive/negative fixtures
against CDDL and JSON Schema and check deterministic CBOR. `conformance/hub/`
will contain language-neutral enrollment, authorization, replay, git, debug,
inbox and credit scripts for the shared future wire harness. Fixture checks do
not claim a real runtime or mock already passes behavioral conformance.

## Approval

Proposed for independent review together with the subsequent spec PR. No v0
freeze or implemented protocol conformance is claimed by this ADR.
