# ADR-0006: Core Wasm ABI and atomic step journals

- Status: Proposed
- Date: 2026-11-23
- Authors: Coglet core specification worker
- Related PR: submitted with this ADR; see PR history
- Supersedes / superseded by: none

## Context

The revised runtime design requires one language-neutral ABI and one replay
artifact shared by the runtime, gate, trace store and simulated visualizer.
The earlier component-model text is superseded by the revised design.

## Decision

Define `spec/abi.md`, `spec/messages/abi.cddl`, the matching JSON Schema,
`spec/manifest.schema.json`, golden fixtures and `conformance/abi/` data cases.
Use wasm32 offsets and deterministic CBOR for host calls; expose a transactional
filesystem and ordered asynchronous effects. Commit a durable journal/outbox
before dispatch; retain recorded inputs and replies so replay never invokes a
live provider. Hash file-tree snapshots and journal records with SHA-256.
Trace headers include the complete per-kind initial committed effect-ID ledger
(including completed effects) and pending inbox-wake indices, alongside pending
timers/LLM requests. Standalone replay reconstructs host conflict returns and
wake consumption without consulting live runtime history. Validate decoded
inbox envelopes/replies and canonical JSONL bytes, not merely snapshot hashes.
The ABI root union excludes internal helper types in both schema languages.

Budget enforcement and cross-engine determinism remain **TBD-by-ADR-0001** and
**TBD-by-ADR-0003** until the engine findings are accepted at freeze. In particular
this proposal does not equate WAMR instruction counts with portable fuel, require
shared-memory AOT, or claim that a watchdog makes replay deterministic.

## Alternatives

Component model: conflicts with the revised design. Synchronous LLM calls:
violate bounded steps. Journaling only hashes: cannot replay without original
inputs, files and replies. Dispatch before commit: leaks effects from failed steps.

## Consequences

Private snapshots and traces contain sensitive state; they stay local unless
explicitly authorized for export. Durable outbox identities support recovery but
do not promise exactly-once effects from an arbitrary external surface/provider.
A runtime cannot claim frozen ABI conformance until the engine TBDs are settled.

## Verification

`python3 -m unittest discover -s spec/tests -v` validates positive/negative
ABI and manifest fixtures against both schemas, golden CBOR and hashes.
`conformance/abi/cases.json` supplies data-only scenarios for the harness owner;
schema validation is not an engine conformance claim.

## Approval

Pending independent review of the ADR and ABI PR. Merge is acceptance of the
specified format only; engine decisions remain explicitly pending.
