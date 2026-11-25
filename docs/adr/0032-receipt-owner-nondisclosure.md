# ADR-0032: Receipt owner nondisclosure

- Status: Proposed
- Date: 2026-11-25
- Authors: Coglet runtime receiver owner
- Related PR: This ADR's accepting PR
- Supersedes / superseded by: None; corrects one frozen fixture expectation

## Context

[The router protocol](../../spec/hub-protocol.md)
states that `not_found` on a receipt means never admitted for that caller/target.
The landed runtime returns the same `not_found` for an unknown ID and an ID owned
by another actor. The conformance README also says not to disclose the foreign
actor's operation. However, `conformance/hub/cross-owner-receipt.json` expects
`permission_denied`, which distinguishes foreign ownership from nonexistence.
The actual authenticated runtime receiver reproduced this mismatch with both
receipt and original-method authority present; withholding a grant would test
a different refusal and cannot qualify receipt ownership.

## Decision

Preserve the protocol and runtime nondisclosure semantics. Correct only the
`error.code` expectation in `cross-owner-receipt.json` from `permission_denied`
to `not_found`. Clarify the existing README provisioning requirement: with fresh
receipt and original-method authority, foreign-owner and unknown receipt lookups
have the same error shape, apart from request correlation. No receipt contents,
owner identity or existence signal may be disclosed.

No wire method, schema, protocol text, runtime semantics, request bytes, case
name, case count or other frozen case changes. The receiver subset remains 25
cases. This docs/fixture-only PR must be independently reviewed and merged through
the queue before the receiver implementation relies on the correction.

## Alternatives

- Return `permission_denied` for a foreign owner: rejected because it exposes
  existence and contradicts the protocol's caller/target interpretation.
- Withhold receipt or original-method grants in the fixture: rejected because
  it exercises missing authority rather than ownership.
- Count the mismatch as a pass or synthesize a reply in the bridge: rejected;
  qualification must drive the actual authenticated receiver.

## Consequences

The fixture now follows the existing nondisclosure rule; implementations already
returning `not_found` need no production change. Callers cannot distinguish an
unknown ID from someone else's ID. This correction is not receiver acceptance:
completed step-zero provisioning, actual transport execution and reopened WAL,
audit and no-disclosure oracles remain required. ADR-0031 filesystem/manage-port
conversion and positive action bridging remain outside this correction.

## Verification

- Compare the JSON against its parent: exactly one expected error code changes;
  every request and every other frozen JSON file remains byte-identical.
- Run `python3 -m unittest discover -s spec/tests -p test_hub.py -v` for fixture
  and schema validation. This does not establish wire behavioral conformance.
- Receiver acceptance subsequently runs `cross-owner-receipt` and
  `unknown-receipt` against the actual receiver with receipt/original-method
  authority, comparing nondisclosing errors and reopening the real receipt WAL.

## Approval

The owning orchestrator ruled in favor of this correction on runtime receiver
card t_01a20dc080fe984c. Independent exact-head PR review and green required CI,
followed by merge-queue landing, are still required for acceptance of this ADR.
