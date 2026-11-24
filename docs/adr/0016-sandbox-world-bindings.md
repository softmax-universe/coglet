# ADR-0016: Release-pinned sandbox world bindings

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet simulator implementer
- Related PR: recorded in the accepting PR
- Supersedes / superseded by: none

## Context

M2 requires Python-SDK gridbot and cartpole surfaces, a Rust guest-SDK
`gridbot-pilot`, and live/replay tests through production `cogletd`.
[Surface protocol](../../spec/surface-protocol.md) and
[operation bindings](../../spec/capabilities/bindings.md) require a trusted,
release-pinned operation catalog. The runtime currently accepts only built-in
speech/expression operations. Advertising an arbitrary simulator schema URI does
not authorize it. M1 is not a prerequisite for this work.

The simulator needs observations as subscribed events, not a new synchronous
guest read import or an invented interpretation of an existing speech event.

## Decision

Add a sandbox capability family to the release-pinned catalog and generated
schemas. Do not alter surface framing, guest imports, authentication, grants,
clock translation, receipt retention, or journal/replay semantics.

The family is `sandbox`, with these closed operation records:

| Entry | Operation | Direction | Input | Successful output |
|---|---|---|---|---|
| `sim.gridbot` | `sim.gridbot_state` | event | gridbot-state | none |
| `sim.gridbot` | `sim.gridbot_move` | action | gridbot-move | empty map |
| `sim.cartpole` | `sim.cartpole_state` | event | cartpole-state | none |
| `sim.cartpole` | `sim.cartpole_force` | action | cartpole-force | empty map |

- gridbot-state: `x`, `y`, `goal_x`, `goal_y`, `width`, `height`, `steps`
  unsigned integers; `done` boolean. Coordinates are bounded to 0..63, dimensions
  2..64 and steps 0..10000. Seeded worlds are empty rectangular grids; moves
  saturate at their boundaries. Reaching the goal ends the episode.
- gridbot-move: `direction` is exactly `north`, `south`, `east`, or `west`.
  North decreases y. A move after termination is refused without an effect.
- cartpole-state: `x`, `velocity`, `angle`, `angular_velocity` signed integers
  in millionths of SI units (angle in radians), `steps` 0..10000, `done` boolean.
  Encoded values are bounded to signed 32-bit integers.
- cartpole-force: `direction` is exactly `left` or `right`, selecting -10 or +10 N
  for one 20 ms integration step. An action after termination is refused.
- Both state records include `state_hash`, the lowercase SHA-256 hex digest of
  deterministic CBOR of the world snapshot without the hash field. The snapshot
  includes the episode seed and complete dynamics/configuration state. Hashes
  are observations, not grants or proof of external effects.

These are nonexclusive simulator capabilities: sandbox instances are separately
supervised and bind one pilot identity. The supervisor still supplies explicit
operation/subscription grants; catalog presence never grants authority. No
physical resource is advertised or controlled. Unknown operations and schema
URIs continue to fail closed. The runtime uses the same input/output validation
for these entries as for the existing catalog, not a profile-map fallback.

The Python world implements seeded reset and bounded deterministic transitions.
Cartpole's numerical method, constants, termination bounds and state rounding
are pinned by its implementation documentation and regression vectors. No
physics-fidelity or cross-language floating-point equivalence is asserted.
Neither surface advances simulation merely because real time passes: each
accepted action advances exactly one step. Initial state and post-action state
are explicit surface events, with the SDK's durable sequence and grant fencing.

The pilot reacts only to gridbot state, chooses a cardinal move toward its
reported goal, and emits an action with a fresh SDK ID and bounded deadline.
The cartpole acceptance controller also runs as guest Wasm (the same package
may contain its small reference policy); no Python test fabricates guest acts.
Build against `wasm32-unknown-unknown` to avoid ambient WASI imports; use the
Rust SDK's unchanged export/host interface and existing R3 publication transform.

## Alternatives

- Reuse speech/expression: rejected; mismatched semantics and resource authority.
- Advertise arbitrary schema URLs or permit generic maps: rejected; bypasses the
  pinned catalog and cannot establish operation-specific validation.
- Use `test.*` or a mock engine: rejected for M2 acceptance; this is a real
  simulator with real compiled guest code, not an M1 fixture profile.
- Wait for M1: rejected by approved sequencing; build against merged main and
  `v0.0.0-rc1` service APIs, without claiming M1 qualification.

## Consequences

Old peers can reject unsupported sandbox operations as before. New catalog
entries do not change existing built-in schemas. Shared generated files and
catalog changes must be regenerated together and minimally rebased. No native
I/O, provider spend, network service, or product adapter is introduced.

Runtime replay re-executes the independently supplied compiled guest against
recorded inputs and compares state hashes; it never feeds recorded guest outputs
back as a substitute. World replay separately reapplies recorded accepted actions
from the recorded seed and checks every observed world hash. These are distinct
checks; neither alone is sufficient for the milestone.

## Verification

Implementation obligations (not claimed passes in this proposal):

- `test_gridbot_seed_and_boundaries`: deterministic reset and clipped movement.
- `test_cartpole_seed_and_episode`: deterministic trajectories, force and terminal bounds.
- `test_world_surface_validation_and_retry`: real SDK negotiation, invalid input,
  denied/revoked grants and byte-identical retry without a second transition.
- `gridbot_pilot_reaches_goal_live_and_replayed`: compiled Rust guest under
  production cogletd/WAMR, SDK surface, goal reached, per-step world hashes and
  independently re-executed runtime state hashes equal.
- `cartpole_episode_live_and_replayed`: real guest/controller and SDK surface
  through cogletd, bounded episode completed, both replay comparisons equal.
- Changed-seed/action/guest negative controls must fail the relevant comparison.
- Generated-code freshness, existing surface/ABI conformance and required CI
  remain mandatory; independent exact-head review and merge precede acceptance.

## Approval

Pending independent review and accepting PR. This proposal alone grants no
execution authority and does not claim simulator or M1 qualification.
