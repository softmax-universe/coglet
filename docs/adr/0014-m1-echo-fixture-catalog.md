# ADR-0014: Isolated M1 echo fixture catalog

- Status: Accepted (PR #73)
- Date: 2026-11-24
- Authors: Coglet M1 integration owner
- Related PR: [#73](https://github.com/softmax-universe/coglet/pull/73); M1 implementation follows separately
- Supersedes / superseded by: none

## Context

The M1 integration must execute the compiled Rust reference coglet in
`coglets/echo` under WAMR, route a surface event through its real `handle` call,
and deliver the resulting action to the Rust mock surface. Its manifest and
code use `test.echo`. The release-pinned runtime catalog correctly rejects that
name. The existing timer-only compiled-guest smoke emits no action and cannot
qualify the required roundtrip.

[Capability bindings](../../spec/capabilities/bindings.md) already permit a
local test-only catalog/grant for illustrative operations while excluding them
from production dispatch. This ADR selects that existing allowance; it changes
no spec text, wire protocol, production catalog entry or production grant rule.
The orchestrator reserved 0014 and selected this approach on M1 card
`t_01a2084823d398be` before implementation.

## Decision

Provide a strictly isolated fixture-catalog seam for `tests/m1`. It must be
compiled only for tests, gated by an explicitly dev-only feature, or available
through a test constructor that neither the production `cogletd` binary nor the
C ABI exposes. No production CLI, environment variable, manifest, schema URI,
peer advertisement or network request may enable or extend the fixture catalog.
The normal runtime constructor keeps the release-pinned built-in catalog.

The fixture harness installs explicit trusted schemas and local grants for
`test.echo`, not a wildcard for `test.*`. The event and action payloads are the
same profile-valid map preserved by the reference echo guest; successful action
result is the closed empty map. Fixture bindings must select event versus action
explicitly and validate those schemas rather than resolving peer-provided URIs.
Authenticated source, target, actor, grant epoch, expiry and retained action
correlation checks still apply. A fixture grant is explicit harness authority,
not authority derived from a valid manifest or a parseable envelope.

The harness must execute the unchanged reference Rust guest compiled to Wasm;
a WAT replacement or native guest surrogate does not qualify M1. It must also
exercise the real hub kernel over iroh. Journal replay must reexecute the guest
offline with recorded inputs and compare its resulting state hash; archive
self-consistency or copying a recorded hash is not replay qualification.

## Alternatives

- Add a production echo operation: rejected because no product needs it and it
  would enlarge the production catalog solely for a fixture.
- Permit arbitrary test operations in the production catalog: rejected because
  parsing is not admission and fixture authority must not escape into production.
- Use runtime.timer-only smoke or a WAT program: rejected because neither proves
  the reference echo event-to-action contract.

## Consequences

Production wire and catalog compatibility are unchanged. Fixture qualification
is limited to the explicitly installed test operation, qualified Linux engine
mode and exercised paths. It does not qualify native speech, remote providers,
EO1/iOS, unpublished transforms or any other fail-closed M1 inventory gate.
The implementation PR will cite this ADR and record exact test/Phase 2 evidence
in the qualification inventory without converting unperformed proofs to passes.

## Verification

The implementation PR must supply these named tests (not claimed executed here):

- `production_runtime_rejects_test_echo`: normal runtime rejects fixture-only
  capability attachment/dispatch, even with a parseable echo manifest.
- `fixture_catalog_requires_explicit_grant`: absent, wrong-actor, expired or
  revoked fixture authority fails closed; valid explicit authority succeeds.
- `hello_coglet`: enrollment through real hub/iroh, Rust mock surface event,
  compiled reference echo/WAMR action delivered, and journal export.
- `replay_state_hash`: offline guest reexecution reproduces the live state hash;
  mutated recorded input/state is rejected or yields a detectable mismatch.

Required existing schema/conformance tests remain unchanged. Linux CI must run
the M1 tests, while the normal production binary and C ABI remain unable to
select the fixture catalog.

## Approval

Accepted through independently reviewed PR #73, merged before M1 implementation.
That decision does not claim implementation qualification or completion of M1 gates.
