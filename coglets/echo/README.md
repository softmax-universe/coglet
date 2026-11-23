# Echo reference coglet

Fixture identity: `coglet:` followed by 64 `a` characters. Receives fixture-only
`test.echo` events and stages one `test.echo` action per event, preserving the
payload and correlation ID. Ignores unrelated/runtime wakes (no receipt loop).
The action expires one second after the recorded runtime logical time; overflow
aborts instead of wrapping. Host rejection aborts the wake.

Build from the repository root with `bash sdk/guest-rust/build-guests.sh`.
`wake` is also a public generic Rust function, so the native mock can execute the
same code through an SDK `Host` adapter; no duplicated guest script is needed.

This manifest is **test-only**: `test.echo` requires an isolated trusted fixture
catalog and grant, not production dispatch. Its compute mode remains pending
alignment/qualification with the engine card. See SDK README proof boundaries.
