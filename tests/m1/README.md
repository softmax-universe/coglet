# M1 Linux integration (ADR-0014)

Run `rustup target add wasm32-unknown-unknown && bash tests/m1/run.sh`.
The explicit `m1-tests` test target is release-only: echo's unchanged source
sets a one-second action deadline. Debug host/WAL overhead does not qualify that
latency. The test uses real clocks and never extends the guest deadline.

## Exercised route

- `hello_coglet` enrolls its harness device with an explicit invite on the real
  `coglet_hub::Kernel` using encrypted iroh and the production ALPN. It keeps that
  session alive while the device's local native runtime runs, and reports the
  observed runtime queue depth with `hub.health`. Missing invite and subsequent
  revocation are rejected. This is enrollment, not deployment or hub-relayed
  surface traffic.
- In that device, the real `RuntimeService`/`NativeSchedule` and `SurfaceSession`
  attach the durable Rust `MockSurface`. The source in `coglets/echo` is compiled
  without edits for `wasm32-unknown-unknown`, memory-bounded and R3-canonicalized
  by the existing production preprocessing helper. This is **not** the separate
  `wasm32-wasip1` release artifact and does not claim WASI qualification.
- A granted `test.echo` surface event executes the compiled guest under WAMR;
  its action reaches the mock backend. The test checks the persisted successful
  empty-map outcome, exports/reads the trace, and reopens the WAL.
- `replay_state_hash` tears down the live runtime/surface and independently
  executes a fresh WAMR engine with the recorded inputs. The resulting final
  state hash equals the live trace's hash. Independently checksum-resealed
  input and output mutations fail with `replay mismatch at step`, rather than
  merely failing archive checksum validation.
- `production_runtime_rejects_test_echo` rejects fixture capabilities through the
  normal constructor. `hello_coglet` also refuses production reopening of a
  fixture-dispatch WAL. Normal catalog globals never change.
- `fixture_catalog_requires_explicit_grant` has positive controls on both sides
  and missing/wrong-actor/expired/revoked negatives, unknown operation/schema
  refusals and action-time expiry/payload negatives.

The fixture catalog is selected only by explicit trusted in-process constructors
per ADR-0014. No daemon CLI, environment variable, C ABI or peer message can
select it. `cogletd` is exercised separately by
`socket_compiled_echo_timer_wake_starts_executes_and_stops`: a real subprocess
loads and executes this compiled guest and persists a timer step. That lifecycle
smoke is **not** the fixture event/action roundtrip. The production daemon
intentionally cannot attach `test.echo`; claiming otherwise would contradict the
accepted isolation contract.

## Qualification boundary

This is a Linux interpreter + local mock route. It does not qualify AOT, native
EO1/iOS, acoustic speech, remote LLM providers, publish transforms, Git deployment,
or every native ABI role. See `docs/reviews/spec-v0-m1-gates.md` for named Phase 2
evidence and remaining refused/unqualified routes. A B gate is not globally
closed merely because this route does not exercise it.
