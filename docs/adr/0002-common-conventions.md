# ADR-0002: Common wire conventions

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet contributors
- Related PR: See this document's introducing PR
- Supersedes / superseded by: None

## Context

Every protocol needs the same identities, deterministic values, correlation,
errors and version negotiation. `spec/conventions.md` is currently a placeholder.
ADR-0001 is reserved for the v0 freeze by `docs/plan.md`.

## Decision

Define `spec/conventions.md` and reusable definitions in
`spec/messages/common.cddl` and `common.schema.json`. Use unsigned 64-bit
counters and signed 64-bit negative values, matching common language bindings.
Use RFC 8949 core
deterministic CBOR (bytewise lexicographic map-key order, not length-first).
Allow integers, text, arrays, text-key maps, booleans and null; omit bytes,
floats and tags in this initial JSON-compatible profile. Identity text is a
role prefix plus the lowercase 64-hex iroh public key. Capability names have
two lowercase components; methods may have more. All envelopes carry a
correlation id; actions also have a distinct id and an accept/outcome lifecycle.
Use strict versioned records, a shared error vocabulary, and select the highest
common explicitly offered major.minor version during hello and enroll.

## Alternatives

- JSON on the wire: debugging-friendly, but has no single deterministic encoding.
- Arbitrary CBOR: forces language bindings to handle tags, float edge cases and
  non-string map keys before any capability needs them.
- Length-first map sorting: not RFC 8949 core deterministic encoding.
- Implicit major-only negotiation: cannot fence additive minor-version fields.

## Consequences

Schemas share one JSON-compatible value model and bounded unsigned counters.
Binary media requires a later explicit schema decision, not a hidden debug-view
conversion. New fields require negotiation; consumers fail closed on unknown
fields at the selected version. Node-key identity is not authorization: transport
binding and method grants remain mandatory in the protocol specs.

## Verification

Golden valid/invalid fixtures under `spec/fixtures/common/` are checked against
both schemas. `spec/tests/test_common.py` tests deterministic golden bytes and
rejection of non-profile encodings. CI must run the same command. These tests
are schema tests, not a claim that a protocol implementation is conformant.

## Approval

Pending independent exact-head review of the introducing PR. Acceptance and
merge are owned by the same-card reviewer; downstream specs wait for merge.
