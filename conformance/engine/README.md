# Engine qualification suite

This suite drives **real implementations over a framed CBOR pipe**, using the
repository's existing black-box harness. It is an engine **test adapter** protocol,
not a new normative guest/runtime wire protocol. The runner imports no engine code.

```sh
cargo build --locked -p coglet-engine-wamr --example wamr-conformance
conformance/run --junit target/engine-registered.xml
# Regenerate committed case data (requires WABT):
python3 conformance/engine/generate_cases.py
```

`adapter.py <engine-json-line-program> [args...]` only bridges deterministic CBOR
(u32be framing, common profile) to JSON lines. It does not choose answers or execute
fixtures. The same case JSON and bridge are used for other engine implementations.

## Adapter contract

Every request is a closed object with these fields:

- `op`: `step`;
- `fixture`: test-only symbolic guest identifier, accompanied by the exact module
  bytes in `module_hex`. Real WAMR always validates/executes those bytes; it never
  dispatches by fixture name. A native mock can bind **verified fixture bytes** to
  an explicitly simulated cooperative closure, without claiming Wasm execution;
- `compute_mode`: `qualified` resolves to the adapter's one explicitly qualified
  mode (reported as `effective_compute_mode`); `aot` and unknown modes must be
  refused, not downgraded. This alias exists only in the test adapter;
- `compute`, `memory_pages`, `host_bytes`, `runtime_bytes`, `stack_bytes`:
  explicit positive limits, with engine-specific compute units;
- `wall_time_us`: relative test request duration; the adapter establishes one
  absolute deadline before load/initialization, and never renews it;
- `events_hex`, `step`, `logical_time`, `seed`: the guest handle arguments.

Each response contains `ok` (bool), nullable `error`, `compute_used`, and
`host_bytes_used`. It may report additional metadata. Stable adapter errors are
`invalid_argument`, `unsupported_mode`, `unqualified_module`, `compute_exhausted`,
`memory_exhausted`, `host_bytes_exhausted`, `deadline_exceeded`, `invalid_pointer`,
`guest_exit`, `guest_return`, `trap`, and `replay_diverged`. Production callers get
the typed `coglet_engine::Error`, not this test serialization.

The bundled cases exercise fresh globals across steps, runaway deadline/recovery,
deterministic cumulative compute exhaustion (including allocation), memory growth
and admission, invalid pointers, forbidden/wrong-signature imports, forbidden start
functions, signed-FFI bounds and unsupported modes. Unit/qualification tests add
initialization/constructors, exact finite cap, all four host imports, restricted
WASI raw i64 wiring, host-byte limits, recomputed replay divergence and NaN policy.

## Frozen AOT disposition

ABI v0 **requires interpreter-only execution** (spec/abi.md:266–271, ADR-0009).
WAMR AOT ignores deterministic metering, and ordinary AOT also lacks safe deadline
interruption. Passing this suite therefore includes **refusing AOT before loading
or instantiation**; no AOT implementation is registered/advertised as qualified.
`cargo test -p coglet-engine-wamr --no-default-features --features aot` separately
proves feature-selected refusal. This intentionally supersedes the original card's
pre-freeze request to execute both modes; it does not weaken frozen limits.

No case claims native EO1 RSS, iOS archive/link/device qualification, real filesystem
transactions, authenticated publish provenance, or runtime journal conformance.
