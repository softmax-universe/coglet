# ADR-0007: Surface transport and portable speech profiles

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet surface spec worker
- Related PR: https://github.com/softmax-universe/coglet/pull/9
- Supersedes / superseded by: none

## Context

The surface and capability placeholders need a normative contract before SDKs
and the runtime are implemented. Common conventions 0.1 already define envelopes
and the deterministic text-key CBOR subset. Native audio must not traverse Wasm.

## Decision

Define `spec/surface-protocol.md` with authenticated local binding, negotiation,
capability discovery, per-coglet fenced grants, read/reply, action admission and
terminal outcomes, health and a surface-owned latched kill switch. Socket frames
use a four-byte big-endian length followed by deterministic CBOR. iOS uses the
same encoded message through an in-process runtime C ABI, without a socket or JIT.

Define `spec/capabilities/speech.md` and `expression.md`: body-neutral semantic
speech events/actions, monotonic transcript revisions and terminal utterances,
immediate native barge-in, configurable STT/TTS placement and provider speech
sessions with tool-call events (never authority to execute tools). Native audio
stays off this control channel; no change to the common binary-data prohibition.

Add imported CDDL/JSON Schema definitions, golden fixtures, scripted audio
and protocol conformance case data with CI tests. This is specification and case
data, not a claim that a surface implementation passes a black-box suite.

## Alternatives

- Product-specific Face messages: prevents the same coglet from using other bodies.
- Raw audio in Wasm or CBOR: violates native reflex timing and the common profile.
- A separate iOS protocol: creates two contracts and transport drift.
- Automatic retry after an unknown effect: risks duplicate real-world actions.

## Consequences

SDKs implement one control protocol. Surface permission checks remain authoritative
on every operation; grants expire and kill stops native I/O independently of the
runtime. Hardware-specific expression rendering is outside coglet decision code.
No exactly-once claim is made across loss of the durable action ledger.

## Verification

`python3 -m unittest discover -s spec/tests -v` validates both schema languages,
golden wire encoding and the surface/speech case data. Existing CI discovers the
new tests; a minimal coordinated Ruby/Bundler setup step installs the locked
reference CDDL validator (the existing pycddl cannot validate recursive maps).
Cases cover grant fencing, kill, reads,
action recovery, barge-in, over-talk and a late partial after utterance close.

## Approval

Pending independent exact-head review of the linked PR; proposed text grants no
implementation or merge waiver.
