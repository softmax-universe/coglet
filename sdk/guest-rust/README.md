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

## Proof boundaries / integration pending

The Python host uses Wasmtime as a supplemental export/guest test engine. It
provides only test directories and captured ABI calls. Its WASI writes are NOT
a transactional filesystem: these tests are not runtime commit/rollback,
budget qualification, mock-engine or WAMR conformance. Both reference manifests
use fixture-only `test.echo` as explicitly allowed by the frozen binding spec;
a production deployment MUST replace fixture identities and install a trusted
catalog, schemas and grants for any extension operations. The manifests'
`wamr-interp-metered-v0` compute mode is a pending integration placeholder, not
qualified engine admission; align it with the engine owner's exact published
mode before claiming runtime conformance.

Required remaining integration: run reference guest cases through the native
mock Engine implementation (and WAMR when merged) under `conformance/abi/`.
The existing `abi.step` timeline is a runtime-script seam, not a Wasm guest
executor, so passing these supplemental tests must not be reported as that
suite passing. Owners: mock t_01a205e53e26e3a6, engine t_01a205e4fdee443e;
SDK t_01a205e62f38d7ca retains acceptance until integration is actually run.
