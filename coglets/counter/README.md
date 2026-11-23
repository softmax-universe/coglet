# Counter reference coglet

Fixture identity: `coglet:` followed by 64 `b` characters. For each wake, adds the
number of delivered events to an unsigned 64-bit decimal `/data/count`, then
stages an info log `count=N`. Missing state starts at zero; malformed state,
integer overflow, file errors and rejected log imports abort. A runtime must
atomically roll back the file write if logging or any later step work fails.

Build with `bash sdk/guest-rust/build-guests.sh` from the repository root. `wake`
is public native Rust as well as a WASI entry: all file I/O goes through the SDK
Host boundary so a native mock supplies a transactional overlay, never an
ambient host `/data` directory. Fresh-instance Wasm tests verify persistence.

The subscription is fixture-only `test.echo`; deployment requires an isolated
trusted fixture catalog/grant. Compute-mode admission remains an engine
integration gate. See SDK README for qualification boundaries.
