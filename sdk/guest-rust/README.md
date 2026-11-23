# Rust guest SDK (ABI 0.1)

Write a synchronous entry point, build a **cdylib** for `wasm32-wasip1`, and
export a bounded unshared memory. No command-style `main`/`_start` is used.

```rust,ignore
use coglet_guest_rust::{coglet, Context, Event, Host, HostError};

#[coglet]
fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    let id = ctx.next_id()?;
    ctx.timer(&id, ctx.logical_time.checked_add(1000).ok_or(HostError::InvalidArgument)?)
}
```

The macro exports `abi_alloc(u32)->u32` and
`handle(u32,u32,u64,u64,u64)->i32`. Unsigned arguments are Wasm bit patterns.
Allocation buffers remain live through `handle`, including when the host makes
multiple allocations. Entry decoding strictly validates deterministic CBOR,
nonempty batches and each common event; malformed inputs return nonzero without
calling user code. All retained buffers are cleared when handle returns. The
engine MUST instantiate afresh for every wake (a returned error is an abort,
not a request to retry the same instance).

`Context` provides `timer`, `emit`, `llm`, `log` with generated types. It validates
and encodes before passing bytes to `Host::call`, then maps 1–4 to typed errors
and preserves reserved status values. Success only means **staged**, never
surface acceptance/provider completion. `llm` accepts only `llm.complete`.
`next_id` hashes recorded seed, trace-local step and local call index to 128 bits;
the runtime still enforces lifetime uniqueness. Trace resets must preserve an
appropriate seed/identity/ID policy; this function is not an exactly-once claim.

Use `Context::read_file`/`write_file` under the ABI's restricted `/code`, `/data`,
`/logs` mounts. WASI implements these with `std::fs`; native mock hosts provide
a transactional overlay (defaults deny access). This keeps the same reference
guest code usable natively without touching an ambient host `/data`. Ordinary
`std::fs` remains available to WASI-only guests. Persistence/rollback, identity/grant checks, inbox validation/cursors,
provider replies and commit/outbox durability remain runtime/application duties.
No synchronous surface-read or network API is exposed by this SDK.

## Build and test

From the repository root:

```sh
cargo test -p coglet-guest-rust --locked
bash sdk/guest-rust/build-guests.sh
uv run --with-requirements sdk/guest-rust/tests/requirements.txt python3 sdk/guest-rust/tests/test_wasm.py -v
```

`build-guests.sh` builds the standalone echo/counter packages to
`target/guests/wasm32-wasip1/release/coglet_{echo,counter}.wasm`, with declared
maximum 256 pages (16 MiB), matching their manifests. Use this driver rather
than an unbounded raw `cargo build`. Rust CI runs it and uploads both artifacts.

Generated validation uses deterministic schema/cache maps and no-std regex on
Wasm: otherwise randomized caches call forbidden `random_get`. The SDK binds
wasi-libc's internal clock syscall symbol to an unconditional Wasm trap, because
the unused futex timeout path otherwise retains a banned `clock_time_get`
import. It never supplies fake time or broadens host access. Tests inspect the
actual modules for forbidden imports and execute the real allocation/entry/host
paths; toolchain changes must keep those tests green. WASI `_initialize` may be
called once by a reactor loader; no start section or `_start` is permitted.

## Proof boundaries

The Python Wasmtime host is a supplemental export test, not runtime qualification.
`conformance/abi/reference-guests.json` is the shared `guest-reference` profile:
exact CBOR requests/responses run through the harness's framed pipe bridge and
`sdk/guest-rust/examples/reference_conformance.rs`. Run it with either `mock` or
`wamr` as the final adapter argument. Both execute the same reference `wake` code:
MockEngine registers fixed opaque module keys and fresh metered native guests;
WAMR loads the actual built Wasm bytes. Neither reads fixture names or expected
responses. Files are an in-memory overlay and failed attempts discard calls and
writes. Counter persistence is checked by supplying the prior committed snapshot
to a fresh instance. This test adapter is NOT the runtime journal/durable WAL,
capability catalog, or a complete WASI implementation. Native compute is explicitly
cooperative simulation, never native preemption/RSS measurement.

```sh
cargo build --locked -p coglet-guest-rust --example reference_conformance
for engine in mock wamr; do
  conformance/run --suite abi --peer-role guest-reference --impl "python3 conformance/engine/adapter.py target/debug/examples/reference_conformance $engine" --junit "target/reference-$engine.xml"
done
```

Non-reference runtime/journal cases are reported as unsupported-role skips, not
passing claims. Existing `abi.step` is still a separate runtime-script seam.
The build driver applies the existing R3 NaN transform before publication; WAMR
independently validates the transformed bytes without an admission exemption.
Both manifests select `wamr-2.4.3-fast-cumulative-v1`; native simulation selects its
own Engine metadata mode rather than claiming WAMR accounting. Manifests use
fixture-only `test.echo`, permitted by the frozen binding spec. Production must
replace fixture identities and install a trusted catalog, schemas and grants.
