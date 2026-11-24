# ADR-0016: Release-pinned sandbox world bindings

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet simulator implementer
- Related PR: https://github.com/softmax-universe/coglet/pull/78
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

Catalog rules use `sandbox:gridbot-state`, `sandbox:gridbot-move`,
`sandbox:cartpole-state`, and `sandbox:cartpole-force`; successful action output
uses existing `surface:empty-map`. Advertisement URIs follow the existing
`https://coglet.dev/spec/messages/sandbox.schema.json#/$defs/<rule>` convention.

- gridbot-state: `x`, `y`, `goal_x`, `goal_y`, `width`, `height`, `steps`
  unsigned integers; `done` boolean. Coordinates are bounded to 0..63, dimensions
  2..64 and steps 0..10000. The producer additionally enforces `x,goal_x < width`
  and `y,goal_y < height`; generic wire schema checks alone cannot prove these
  cross-field invariants. Seeded worlds are empty rectangular grids; moves
  saturate at their boundaries. Reaching the goal ends the episode; a 10000-step
  cap also ends it, with `done` indicating termination (not necessarily success).
- gridbot-move: `direction` is exactly `north`, `south`, `east`, or `west`.
  North decreases y. A move after termination returns `accept=false` with
  `conflict` and no outcome/effect.
- cartpole-state: `x`, `velocity`, `angle`, `angular_velocity` use common
  `profile-int` constrained to [-2147483648,2147483647], not CDDL `nint` (positive
  and zero values are valid). Units are millionths of SI (angle in radians).
  `steps` is 0..10000 and `done` boolean.
- cartpole-force: `direction` is exactly `left` or `right`, selecting -10 or +10 N
  for one 20 ms integration step. An action after termination returns
  `accept=false` with `conflict`, and no outcome/effect.
- Both state records include `state_hash`, the lowercase SHA-256 hex digest of
  deterministic CBOR of the closed world snapshot specified below. Hashes are
  observations, not grants or proof of external effects.

The closed gridbot snapshot has exactly `world="gridbot-v1"`, `seed` (uint64),
`width`, `height`, `x`, `y`, `goal_x`, `goal_y`, `steps`, and `done`, with the
same field bounds as the state event. The closed cartpole snapshot has exactly
`world="cartpole-v1"`, `seed` (uint64), `max_steps` (1..10000), `x`, `velocity`,
`angle`, `angular_velocity`, `steps`, and `done`, with the same state bounds.
There is no retained mutable RNG: reset derives initial state from seed, while
transitions use only this snapshot and the action. Dynamics/constants are pinned
by the `world` version; changing them requires a new version. The snapshot does
not contain `state_hash` itself. Extra keys are forbidden.

These are nonexclusive simulator capabilities, not physical resources. Each
sandbox supervisor MUST bind exactly one coglet in both the runtime's
`SurfaceBinding.permitted_coglets` and Python `Surface.coglets`, and only grant
that identity. Simulator setup rejects zero/multiple pilots. This is a trusted
supervisor invariant, not a claim that an empty resource list enforces ownership.
Unknown operations and schema URIs continue to fail closed. The runtime uses the
same input/output validation for these entries as for the existing catalog,
not a profile-map fallback.

Cartpole-v1 uses 1,000,000-scaled fixed-point arithmetic; multiplication and
division truncate toward zero after each operation. Its equations are the
standard frictionless cartpole accelerations with cart mass 1 kg, pole mass
0.1 kg, half-length 0.5 m, gravity 9.8 m/s². Sine/cosine are evaluated with the
fixed-point polynomials `sin(t)=t-t³/6+t⁵/120` and
`cos(t)=1-t²/2+t⁴/24`, with fixed-point multiplication after each power and
truncating division for terms. Explicit Euler updates position and angle from
old velocities, then velocity and angular velocity from acceleration, with a
20 ms step. Termination occurs at |x| > 2.4 m, |angle| > 0.209440 rad or
`max_steps`; there is no transition after termination. Intermediate integers
use arbitrary precision. This is a reproducible approximate simulator, not a
claim of physical fidelity. Reset uses a specified portable 64-bit LCG
(`s = (6364136223846793005*s + 1442695040888963407) mod 2^64`), taking four
successive `s % 100001 - 50000` samples for x, velocity, angle, angular velocity.
Gridbot reset takes four successive LCG states modulo width,height,width,height
for x,y,goal_x,goal_y; if start equals goal, set goal_x=(goal_x+1)%width.

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
