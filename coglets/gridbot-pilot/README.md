# Gridbot pilot

Rust guest-SDK reference policy for `sim.gridbot_state`: move horizontally to the
goal, then vertically; emit nothing after terminal state. The same guest includes
a deterministic small PD controller for `sim.cartpole_state` (four times angle
plus angular velocity). Non-world wakes are ignored. Each action uses SDK-derived
IDs and a ten-second logical deadline; the compiled pilot identity is `coglet:`
followed by 64 `b` characters, matching the supervisor's singleton binding.

Run `bash sim/check-native.sh` at repository root for policy tests, a bounded
`wasm32-unknown-unknown` build, R3 publication transform, and actual cogletd
live/replay tests. This target intentionally has no WASI imports: the production
runtime's ambient-WASI refusal remains intact. See [sandbox documentation](../../sim/README.md)
for determinism, proof and lifecycle limits.
