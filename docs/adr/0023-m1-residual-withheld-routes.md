# ADR-0023: M1 residual qualification disposition

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet M1 residual qualification owner
- Related PR: M1 component qualification [#120](https://github.com/softmax-universe/coglet/pull/120); accepting ADR PR recorded in Approval
- Supersedes / superseded by: none; supplements ADR-0009 and ADR-0014

## Context

[ADR-0014](0014-m1-echo-fixture-catalog.md) accepts a bounded compiled-echo,
local mock, authenticated iroh enrollment and offline replay proof. It neither
qualifies every B row in the [M1 ledger](../reviews/spec-v0-m1-gates.md) nor
permits production `cogletd` to run `test.echo`. Residual owner
`t_01a20c2f7b9bb95a` found that the current router exposes event/action
boundaries, not the positive publication, ownership-transfer and private-read /
back-channel interfaces needed to qualify those broader obligations.

The orchestrator reserved 0023 and selected this disposition in that card's
ruling 246808 (reconciled after 246807). The owners below are separate open
successors, gated on completion of the residual card. This ADR changes
qualification ownership, not the [router protocol](../../spec/hub-protocol.md),
wire schemas, grants, catalogs or implementation interfaces.

## Decision

Keep the following positive routes **WITHHELD** until their prerequisites and
named qualification proofs pass. A refusal is never positive qualification.
An implemented component, signed provenance fixture or parseable request does
not authorize advertising any withheld route.

| Withheld route | Current fail-closed boundary and limited evidence | Producer prerequisites and live positive owner |
|---|---|---|
| H-transform: qualified publish-time rewrite before build/commit, pinned-transform fuzz/cross-architecture publication and authenticated provenance admission | No qualified publication pipeline is exposed by the reference router. `Kernel::operation` rejects otherwise schema-valid unsupported methods with `Error::Denied`; malformed requests are `Error::Invalid`. `validator_code_requires_valid_wasm_and_canonical_output_bound_provenance` checks invalid Wasm / canonical output-bound provenance, not an actual rewrite or publication. | `t_01a20d98a7fb7229`: identify the pinned real transform and build/commit publication producer; specify reproducible fuzz corpus and cross-architecture replay evidence, authenticated publisher/target/output binding, and refusal before unqualified publication. Any new or changed interface requires a future independently reviewed ADR before implementation. Require `rewrite_before_build_commit` and `unqualified_transform_publication_refused` plus named pinned-transform fuzz/cross-architecture and authenticated-publication tests in required CI. |
| H-private: ownership-transfer purge/fence | The configured registry does not expose an ownership-transfer operation. Another registration cannot widen an established private owner: `private_speech_owner_cannot_be_widened_by_another_registration`. There is no transfer-positive proof. | `t_01a20d98a8bca347`: a future ADR must select the ownership-change interface and durable epoch/purge/fence semantics before implementation. Require old-owner leakage negatives across transfer/reconnect/restart and a new-owner positive; enrollment or subscription alone must not convey authority. |
| H-private: private reads and debug/back-channel observations | The reference router has no qualified private-read or back-channel serving implementation. This is an implementation gap, NOT a missing frozen protocol: `debug.read`, `debug.write`, `debug.pause` and `debug.step`, actor/ticket context, separate grants and owner-only private relay are already specified in `spec/hub-protocol.md`. Unsupported schema-valid operations at this implementation boundary are denied; malformed ones are invalid. Event-delivery privacy is not proof of read/debug privacy. | `t_01a20d98a8bca347`: implement and qualify the existing frozen debug/owner-only relay contract and retained owner/epoch/correlation checks; no new ADR is required merely to implement that contract. Only a new or changed interface or semantics needs a future ADR; ownership transfer is separately gated above. Require `relay_private_owner_only`, `relay_subscription_not_authority`, private-read and back-channel owner positives with foreign-owner, subscription-only, revoked and stale-owner leakage negatives in required CI. |

`unqualified_transform_publication_refused` is a **required successor proof
name**, not an existing executed test: the current tree contains the provenance
validator test above, not that name. This distinction prevents a proposed test
from becoming a fabricated CI receipt. Existing refusal slices remain required
but do not qualify the missing pipeline or private interfaces.

Move the broader **all-method relay positive matrix** to
`t_01a2084063ee0c84`, full router conformance against both reference and mock,
with no skipped acceptance cases. It owns integration proof, not implementation
of missing producers. Registry/relay/deduplication/audit/sender-mapping
implementation ownership is unchanged. Joined production deployment requires
`t_01a20bcfe6db20c3` (reference-router deploy pipeline) and
`t_01a20bd00f035b09` (`cogletd` lifecycle). Missing device lifecycle,
publication or back-channel producers remain incomplete, never synthetic
positive replies.

PR #120 qualifies only callable router event/action authority and retained
runtime retry boundaries. It must map each named ledger obligation to actual
required-CI test execution, preserving the distinction between direct trusted
contexts and authenticated iroh tests. None of these moves closes unrelated
engine, provider, native speech or device qualification gates.

## Alternatives

- Claim the echo proof or provenance fixtures qualify publication/private reads:
  rejected; those routes were not executed.
- Invent transfer, read, publication or back-channel methods inside a test:
  rejected; a test cannot select a new production protocol.
- Leave obligations assigned to the completed original router-protocol card:
  rejected; a done card cannot deliver outstanding positive qualification.
- Implement all missing producers in this residual card: rejected; they require
  separately selected contracts and their own reviewed implementation scope.

## Consequences

No spec text, wire compatibility, production fixture admission or authority
changes. Unqualified routes remain unavailable; ownership is explicit without
pretending that a negative proves a positive. The two successors must first
obtain any necessary future ADR, not infer new protocol authority from this
qualification disposition. The residual card can finish only after this ADR
lands, PR #120 receives independent exact-head review and required CI and lands
through the merge queue, and the ledger records every scoped obligation as
proven or explicitly moved to an open owner.

## Verification

The required `rust` workflow runs `scripts/ci/cargo-test --workspace --locked`.
Existing bounded evidence to retain:

- `validator_code_requires_valid_wasm_and_canonical_output_bound_provenance`
  (`coglet-proto`, `bundle_validator`): validation, not real transform proof.
- `private_speech_owner_cannot_be_widened_by_another_registration`
  (`coglet-hub`, `provisioning`): immutable registry privacy, not transfer/read.

PR #120 adds `m1_retry_crash`, `m1_authority_matrix` and `m1_synthesis` targets;
its evidence map names each executed case and limitation. Proposed successor
proofs above are **unexecuted obligations**. Neither this documentation PR nor
an offline link check constitutes their qualification. Required docs CI checks
local links; independent review verifies the disposition and owner mapping.

## Approval

Proposed under the orchestrator's accepted disposition; formal acceptance is
through this ADR's independently reviewed, merge-queued PR. No implementation
qualification or merge is claimed by this proposed record.
