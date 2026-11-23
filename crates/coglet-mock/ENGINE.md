# Native simulation engine

`coglet_mock::engine::MockEngine` implements the shared `coglet_engine::Engine`.
Register exact opaque module bytes with a `NativeModule::new(initial, maximum,
factory)`. Each `Instance::handle` constructs a fresh `NativeGuest` from the
factory inside the original deadline. `ClosureGuest` adapts a Rust `FnMut`; a
custom `NativeGuest` can additionally simulate initialization and allocation.
`load`/`instantiate` never execute a factory or guest. A module's initial and
maximum pages must both fit the admitted limits, including re-instantiation
with different limits. Unknown bytes and unsupported modes fail closed.

## Trust and accounting

This is **not a Wasm interpreter, security sandbox, native preemption mechanism,
or production engine qualification**. Trusted factories, closures and Host
implementations must not block, access ambient I/O or hide mutable per-instance
state in shared captures. Put per-step state in the fresh guest. The consuming
Instance prevents ordinary instance reuse; Rust cannot prevent a trusted factory
from sharing an Arc or accessing process globals.

The sole mode is `coglet-simulation-explicit-v1`. All phases share one StepBudget
and one absolute deadline. Charge native work with `Context::charge`; safe points
check elapsed time. Metadata reports `deadline_interrupt=false`. A runaway native
loop that does not call a safe point cannot be interrupted. The compute admission
envelope deliberately uses positive signed-i32 units like the shared suite, but
units are explicit native simulation work, not WAMR instructions. No cross-engine
usage or cutoff equality is claimed.

Linear memory is zeroed per step and bounded in 64 KiB pages. Guest reads/writes
are checked copies; growth beyond the declared maximum returns -1 unchanged.
`reserve_runtime` charges simulated non-linear storage cumulatively;
`check_stack` checks a caller-declared peak. These **do not bound native RSS,
allocator overhead, or the actual Rust call stack**.

Event copy-in and each Host-visible GuestMemory read/write debit the same
aggregate host-byte allowance (including WASI result writes). Invalid pointers
abort; Host-memory faults latch even if a Host ignores its Result. Native guest
memory operations themselves are local simulated operations, not host I/O.
`coglet_v0` import names, argument widths and ranges are checked before Host
entry. `proc_exit`, including zero, unconditionally aborts before Host policy.
Other host operations and restricted WASI/file policy belong to the runtime's
trusted Host; no ambient filesystem implementation is provided here. Callers
must stage transactional effects and discard them on failed StepOutcome, as the
shared trait requires. Nonzero return, explicit errors and unwinding panics fail
the step; `panic=abort` cannot be caught.

## Shared black-box suite

```sh
cargo test --locked -p coglet-mock
cargo build --locked -p coglet-mock --example mock-conformance
conformance/run --suite engine --impl 'python3 conformance/engine/adapter.py target/debug/examples/mock-conformance' --junit target/mock-engine.xml
```

The registry uses the same `conformance/engine/cases.json` and CBOR bridge as
WAMR. The test-only adapter's `qualified` alias resolves explicitly to simulation
mode and reports `simulation=true`. `examples/native-fixtures.json` pins only
exact **input** module bytes; handwritten native guests implement fresh globals,
cooperative runaway work, allocation work, checked growth, and bad pointers.
Execution selects by bytes, never the incoming symbolic fixture name. Unknown,
mutated, forbidden-import/signature/start bytes have no registered native guest.
This is refusal by exact native allowlist, not proof of Wasm structural checking.
The adapter never reads expected responses or case outputs. AOT requests are
refused, never executed or advertised as qualified.

Tests in `tests/engine.rs` independently exercise factory admission, fresh closure
state, full-width inputs, cumulative phase charges, deadline preservation, memory
and host-byte bounds, all four imports, explicit runtime/stack reservations,
panics and recovery. Shared-suite success qualifies these simulations only; it
does not replace the real engine's M1 gates.
