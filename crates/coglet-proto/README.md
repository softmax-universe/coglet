# coglet-proto (implementation in progress)

The authority is `spec/hub-protocol.md` at `spec-v0`. This crate is a clean
implementation; it contains no product or legacy protocol code.

**Not a conformance claim or production-ready server.** This first milestone
implements wire primitives and unary operation transport. The full reference
server, mock hub and registry claims are not yet implemented.

## Integration API (provisional)

- `wire::{encode, decode}` wrap the shared `coglet-types` deterministic CBOR
  profile. `wire::{read_frame, write_frame}` enforce the 1 MiB u32be frame bound.
- `signing::{preimage, verify}` verify the frozen domain-separated Ed25519
  statements against a **caller-selected** purpose and pinned signer. The caller
  separately checks hub/target/device binding, expiry, generation and ref policy.
- `transport::{ALPN, Role, Session, OperationHandler, RequestContext}`:
  construct a `Session::new(connection, local_endpoint.id(), Role)` after the
  iroh handshake. The device must dial its configured pinned hub endpoint.
  `peer()` returns the transport-authenticated remote key; do not accept a key
  from request params as identity. `request(Value)` opens a new bidi stream.
  Concurrently run `session.clone().serve(Arc<dyn OperationHandler>)` to receive
  operations from the opposite endpoint, including hub-to-device operations on
  the device-opened connection. Unary operations require FIN on the request.
- `OperationHandler::handle(RequestContext, Value)` returns a boxed Send future
  producing a terminal wire frame. The context has authenticated `peer`, local
  node `local` and monotonic `deadline`. The handler must atomically commit
  invite/allowlist/grants before returning enrollment success. Negotiation is
  **not** authorization. The handler owns method/target/actor/ticket/delegation
  checks, durable receipts and operation effects. Handler output is schema- and
  method-result-checked before transmission.
- `transfer::Receiver` tracks cumulative credit, contiguous offsets, correlation,
  bounded hex chunks and complete transfer SHA-256. Call `consumed()` only after
  all accepted bytes have reached durable bounded quarantine storage. This is
  accounting, not Git validation or a durable storage implementation.

Kernel integration must keep registry, relay batch dedupe/audit, private-event
routing and immutable once-mapped sender action envelopes in `coglet-hub`.
No fixture-only methods may be added to the ALPN.

## Tests executed for this milestone

```sh
cargo fmt -p coglet-proto --check
cargo test -j 2 -p coglet-proto
cargo clippy -j 2 -p coglet-proto --all-targets -- -D warnings
```

The ten tests exercise frozen common/hub golden CBOR, frozen invalid profile
and framing vectors, uint64/int64 limits, oversize rejection before reading a
body, frozen signing preimages/signatures and tampering, raw-byte transfer
credit/durability/hash accounting, real encrypted iroh enrollment gating and
reverse operation streams, method direction, and enrollment identity binding.
These are component tests, **not** the language-neutral conformance suite.

## Remaining before acceptance

1. Durable atomic one-use enrollment, exact grants/delegations/context scopes,
   expiry/revocation and rate limiting; integrate with the kernel, not a second
   independent policy authority.
2. Per-peer operation/git/decoded-byte bounds, two unclassified-stream budget,
   reserved control slot/fair scheduling, deadlines and application-code-1 reset
   on every invalid/timeout path. Current transport tasks are not yet bounded.
3. Git push/fetch multi-frame operation integration (the unary API does not
   implement git transfers), sender credit validation, signed metadata/watermark
   checks before disclosure/credit, bounded Git quarantine/fsck/archive/provenance
   verification, durable CAS/watermarks and boundary activation.
4. Reference hub/device test endpoints and `coglet-mock::hub`, lifecycle/debug/
   inbox/action/relay behavior with durable receipts and stateful observations.
5. Same `conformance/hub/` cases/oracles against reference and mock using the
   harness's iroh-driver. The current upstream driver is dial-only; the hub suite
   README explicitly requires accept/reverse-stream, reverse-fetch mapping and
   post-run durable oracles before advertising the corresponding fixtures.
   Missing oracle coverage must remain SKIP/incomplete, never PASS.
6. Independent exact-head review, green CI, merge and board review handoff.
