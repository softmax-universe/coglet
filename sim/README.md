# Seeded sandbox worlds

`coglet_sim.worlds.Gridbot` and `Cartpole` are action-clocked, integer-only world
models. `coglet_sim.surface.WorldSurface` exposes either through the Python
surface SDK with the release-pinned `sim.gridbot_*` / `sim.cartpole_*` contracts
(ADR-0016, PR #78). No custom protocol envelope or permissive schema fallback.

```python
from coglet_sim.worlds import Gridbot
from coglet_sim.surface import WorldSurface, runtime_binding

pilot = 'coglet:' + 'b' * 64  # reference guest's compiled identity
identity = 'surface:' + 'a' * 64
world = Gridbot(42)
surface = WorldSurface(identity, [pilot], 'fresh-episode.sqlite', world)
binding = runtime_binding(identity, [pilot])
```

The supervisor authenticates a local peer, negotiates, grants the world's two
operations, and calls `surface.publish(epoch=1)` to start. Every admitted action
advances the world once and publishes the next state. SDK durable receipts make
byte-identical act retries harmless. Both SDK construction and runtime capability
admission require a singleton supervisor-bound pilot; no lease race decides an
owner. Invalid, expired, ungranted and post-terminal acts have no world effect.

## Determinism

Reset uses a specified 64-bit LCG, never Python random or wall time. Gridbot uses
bounded cardinal moves, clips at edges, stops at the goal or 10,000 moves.
Cartpole uses fixed-point frictionless cartpole equations (scale 1,000,000),
truncated integer arithmetic and a fixed 20ms simulation step. It is a portable
sandbox model, **not** bit-equivalent to Gym's floating-point CartPole. It ends
at the track/angle limits or `max_steps` (default 500). The closed, versioned
snapshot includes seed and all configuration/transition state. `state_hash()` is
SHA-256 of core-deterministic CBOR of that snapshot (not the event envelope).

## Proof

```sh
uv run --project sim --locked pytest sim/tests -q  # fast unit/contract tests
bash sim/check-native.sh                         # real compiled guest and daemon
```

The latter builds the Rust guest to bounded `wasm32-unknown-unknown`, applies the
existing R3 float transform, and runs these **unskipped** tests in the Rust CI job:

- `test_gridbot_pilot_reaches_goal_live_and_replayed`: seed 42 reaches its goal.
- `test_cartpole_episode_live_and_replayed`: seed 42 completes a configured
  16-action episode under the guest PD policy. This is a finite integration run,
  not a 500-step balance-performance claim.

Both authenticate the actual cogletd PID/UID over AF_UNIX, use real SDK frames,
wait for an ordered final health round trip, reopen the durable daemon WAL, and
require a committed WAMR invocation consuming the terminal observation. The
standalone `sim/native` tool independently invokes `NativeEngine` for every
recorded input and compares runtime state hashes, effects and deterministic
usage; recorded outputs never substitute for guest execution. World replay
separately reapplies emitted actions from the seed and compares **every** world
hash. Changed world seed/action, changed runtime seed and changed executable pin
are negative controls. Host wall duration is measured/enforced separately and is
not deterministic replay state. The fast Python job explicitly skips just the two
native tests; the Rust job supplies all built executables and executes them.

## Lifecycle boundary

Every episode requires a fresh ledger. Reopening an existing ledger is refused:
SDK action receipts are durable but world state is deliberately in-memory. A
crash during an effect does not support world continuation or automatic replay
of physical effects. Start a new episode/ledger instead. This proof is local
Linux sandbox integration, not hardware, arbitrary guest, reconnect/crash
recovery, signed deployment, or cross-engine qualification. Test setup bootstraps
the ABI fixture configuration and pins the built wasm; it is not a production
source-build attestation or hub enrollment path.
