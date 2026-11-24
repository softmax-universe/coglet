# Mock hub (part 1)

`coglet_mock::hub::MockHub` runs the real `coglet-hub::Kernel` and
`coglet-proto::transport::Session` over encrypted iroh on `coglet/hub/0`.
It owns isolated temporary state, a loopback endpoint, and bounded serving tasks.
This is test infrastructure, not a deployable hub: default keys and trusted Unix
clock are fixed fixture values. Enrollment invites remain OS-random and one-use.

- `start()` creates the standard fixture hub with enrollment closed.
- `with_clock(key, clock)` supplies a different deterministic identity/clock.
  Concurrent independent in-process fixtures should use distinct identities:
  shared transport scheduler limits intentionally span all sessions for a
  `(local, remote)` identity pair.
- `invite(device_id, ttl, grants)` opens a window and creates a peer-bound invite.
- `addr()` supplies an iroh `EndpointAddr`; `id()` is the pinned hub identity.
- `kernel()` exposes trusted local test controls (including `status()` and
  `revoke_peer()`), never additional wire RPCs.
- `shutdown().await` closes connections and serving tasks. Drop aborts serving.

## Native-client test launcher

Build `cargo build --locked -p coglet-mock --example hub`, then start
`target/debug/examples/hub DEVICE_NODE_ID` with stdin/stdout pipes. Supply the
client's **public** endpoint ID, not its secret key. Read one JSON line:

- `hub`: pinned hub endpoint ID;
- `addresses`: local socket addresses for an iroh endpoint address;
- `alpn`: `coglet/hub/0`;
- `device`: the supplied public endpoint ID;
- `invite`: a private 60-second one-use enrollment secret bound to that device.

Do not log the private bootstrap line. Keep stdin open while testing. Close stdin
or send SIGINT to stop. A client must open its device endpoint with the same
identity, dial the supplied endpoint address/ALPN, and send `hub.enroll` with
version offer `0.1` and the invite. On a fresh connection after disconnect, send
`hub.enroll` with null invite to recover the allowlisted identity. Revoked and
unknown peers cannot recover. No Git/credit/quarantine behavior is implemented
by this module; those remain separate integration scope.

## Verification

For bounded disposable build footprint, without changing assertions or overflow
checks, set `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
CARGO_PROFILE_TEST_DEBUG=0` for all commands in a verification run.

```
cargo test --locked -p coglet-mock --test hub
python3 crates/coglet-mock/test_hub_launcher.py
python3 crates/coglet-mock/run_hub_conformance.py
cargo test --locked --workspace -j 2
cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings
cargo fmt --all -- --check
```

The conformance runner currently proves only `enroll-reconnect`,
`enroll-unsupported-version` and `health-minimal`, using unchanged suite JSON and
real iroh. It is explicitly **not complete part-1 conformance acceptance**:
fixed-invite enrollment cases await a producer-owned test-control provisioning
API. Never translate fixture invite bytes, or claim an unknown-token rejection
proves correctly provisioned expiration/window tests.
