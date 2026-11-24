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
- `authority::{Authority, Grant, Scope}` provides a SQLite WAL/FULL enrollment
  transaction and exact-grant intersection layer. Open once per hub and clone
  across sessions (including all enrollment rate counters); do not open a fresh
  authority per connection. Operator calls open/close monotonic windows, generate
  256-bit one-use invites, and provision explicit grants. Only the invite hash is
  stored. `enroll(authenticated_peer, invite)` commits allowlist, grants and secret
  consumption together. Null invite recovers a committed success. Restart closes
  the window and invalidates outstanding unconsumed invites; enrolled peers and
  grants survive. Default attempt limits are 8/peer/minute and 256/hub/minute.
  `authorize` checks exact peer/method/target, trusted Unix expiry and generation.
  `provision_scope` and `authorize_scope` additionally bind exact actor and optional
  ticket, without treating absent ticket as a union. `end_ticket` persists a
  terminal marker; `revoke_peer` removes all peer/scoped grants and invalidates
  outstanding invites. Authorization is a check, **not** a lease or atomic kernel
  boundary application; callers repeat it at every disclosure/transfer/effect.
  `accept_delegation` verifies the coglet-key signature, hub/device/target binding,
  trusted expiry and exact methods. It pins an initial statement or retries the
  identical active statement; no valid signature can replace a current binding.
  `replace_delegation` is **local operator approval only** and atomically requires
  a strictly higher uint64 generation. `revoke_delegation` retains a durable
  tombstone (an absent binding returns permission_denied, not a claimed fence).
  `revoke_peer` also tombstones its current device bindings, so re-enrollment
  cannot restore those statements. State is scoped by (hub,coglet,delegation),
  survives restart and never wraps generation. `authorize_delegation` rechecks
  current binding, generation, method and trusted expiry. Its generation is
  independent of the enrollment/scoped grant generation, not implicitly equal.
  Supply a trusted clock callback returning current Unix seconds or None; it is
  sampled again before registration commit, after any transaction lock wait.
  Do not provide a cached request timestamp or call back into this Authority.
  These APIs are synchronous: production dispatch must use **bounded** blocking
  execution, never block an async reactor on SQLite. A successful check remains
  no lease: kernel integration must serialize rechecks with application/dispatch.
  This does not yet implement per-grant replacement/revocation, kernel policy,
  path/ref/surface limits, or atomic cancellation of kernel-owned pending writes.
  Those remain mandatory integration work. All provisioning/replacement is local
  operator configuration, never a new remotely self-authorizing method.
- `transfer::{Receiver, Sender}` track cumulative raw-byte credit, contiguous
  offsets, correlation, bounded hex chunks and terminal state. The receiver
  checks the complete transfer SHA-256. Call `consumed()` only after accepted
  bytes have reached durable bounded quarantine storage.
- `transfer::{send_chunks, receive_chunks, DurableSink}` implement the raw data
  phase on framed async streams, with at most 32 KiB per chunk and a 64 KiB
  outstanding window. A failed durable sink does not replenish credit. **These
  are not complete Git operations:** the owner must verify signed metadata and
  watermarks before the first credit, enforce deadlines and reset-on-abandon,
  check FIN/trailing frames, validate quarantined Git/archive/provenance, recheck
  authority/watermarks and commit durable refs/receipts before terminal success.
  Session streaming dispatch is not implemented yet.
  `quarantine::BundleSink` supplies a private bounded raw bundle file, with
  <=32 KiB writes fsynced off-reactor before returning. The limit is checked
  before writing; I/O failure poisons the file. Drop removes its generated
  quarantine directory; an in-flight worker retains cleanup custody after future
  cancellation. Callers MUST discard the sink after cancellation (never resume
  that append/transfer). Startup orphan cleanup, Git validation/finalization and
  expanded-object/disk/CPU limits are still missing. This raw sink does not
  establish secure Git quarantine or code admission.
  Raw data-phase tests exercise a 150000-byte transfer and stalled durable
  consumption over real iroh on the hub ALPN, plus sink failure and source EOF.
  They do not establish push/fetch/restart/replay acceptance.
- `git_metadata::{RefKind, VerifiedRef}` pins signed metadata to the requested
  purpose/ref and trusted hub/coglet. Hub refs use the hub key; published/logs
  refs use the coglet key, never the device key or untrusted signer. Pure
  `check_observation` distinguishes higher-generation snapshots, identical
  equal-generation statements, and conflicts (including equal-tip but different
  statements); `check_authoritative` requires exact old tip and higher generation.
  These prechecks DO NOT persist or accept transfers, and signature verification
  is not a grant. The owner must intersect current authority/delegation, compare
  its latest durable watermark before credit, verify the entire quarantine even
  on Identical, and repeat comparisons in the durable commit transaction.
  Tests cover foreign hub/target/purpose/ref/key, tampering, skipped generations,
  uint64 maximum, lower/equal-different conflicts and authoritative old-tip CAS.

Kernel integration must keep registry, relay batch dedupe/audit, private-event
routing and immutable once-mapped sender action envelopes in `coglet-hub`.
No fixture-only methods may be added to the ALPN.

## Tests executed for this milestone

```sh
cargo fmt -p coglet-proto --check
cargo test -j 2 -p coglet-proto
cargo clippy -j 2 -p coglet-proto --all-targets -- -D warnings
```

The component tests exercise atomic SQLite rollback, concurrent one-use invite
redemption, expiry after transaction-lock contention, restart/closed-window
fencing, exact grants, ticket omission/ending, signed delegation binding,
operator-only monotonic replacement with cross-handle concurrency, durable
revocation/re-enrollment tombstones, expiry/clock-loss transaction rollback,
uint64 exhaustion without wrap and live binding replacement on real iroh,
as well as frozen common/hub golden CBOR, frozen invalid profile
and framing vectors, uint64/int64 limits, oversize rejection before reading a
body, frozen signing preimages/signatures and tampering, raw-byte transfer
credit/durability/hash accounting, real encrypted iroh enrollment gating and
reverse operation streams, method direction, and enrollment identity binding.
These are component tests, **not** the language-neutral conformance suite.

## Component acceptance and follow-up ownership

This baseline is milestone 1 only: framing/signing, bidirectional unary iroh
transport, transfer accounting, and durable enrollment/grant/delegation APIs.
Acceptance is the focused component tests above, strict Clippy and formatting,
independent exact-head review, and green CI before merge. It is **not** acceptance
of the complete hub protocol. Remaining work is serialized on the board:

- `t_01a2083fe588e786`: bounded per-peer scheduler, rates and reset paths.
- `t_01a2084010cc02e1`: streaming Git, credit, quarantine and durable watermarks.
- `t_01a205e767bb96f0`: kernel registry, relay/dedupe, audit and sender mapping.
- `t_01a2084063ee0c84`: reference/mock integration and harness extensions; owns
  the original unchanged same-suite hub acceptance with zero skipped cases.

## Scheduler component

`Session::new` automatically shares admission across all connections for the
same (local endpoint ID, authenticated remote peer). Sixteen streams total
include provisional classification; at most fifteen may be bulk, two Git, and
two unclassified. The incoming QUIC bidi capacity is set to sixteen on every
connection, independently of the shared application limit. Classification has a
two-second complete-frame timeout even before enrollment. There is no waiting
application queue: a classified saturation returns retryable resource_exhausted
before handler dispatch; extra/stalled unclassified streams reset with code 1.

Decoded request accounting conservatively charges the compact JSON diagnostic
size, including escaping, twice on ingress (retained request and handler copy).
The cap is 4 MiB with 64 KiB reserved from bulk for control dispatch. The two
provisional raw frames are separately bounded to 1 MiB each. Control methods are
hub.stop/cancel/health/receipt. Health, enrollment and other control use separate
fixed one-minute windows (120/32/240 admissions respectively); these numbers are
local policy, not standardized wire values. Rate history survives disconnects
until the window expires. The identity cache fails closed at 4096 entries rather
than evicting live rates. The Authority's stricter enrollment limits still apply.
RAII guards release admission and use application code 1 for abandoned stream
halves, including timeout/cancellation/panic; valid terminal failures remain
ordinary protocol replies, not malformed-stream resets.

Verified component tests: scheduler caps and reconnect-rate regression; real-iroh
fifteen-bulk saturation with stop/cancel dispatch; two-slot classification/timeout
before enrollment; malformed frame resets; two-Git saturation and deadline
recovery; decoded-byte saturation below the stream cap with cancel dispatch;
separate health/enroll/control rates; handler panic and serve-abort release;
and caller timeout/cancellation/malformed/trailing replies with observed code 1.
The saturation cases use two distinct connections for one authenticated peer.
The mixed-control provisional-slot regression additionally checks that slot 16
can never become bulk even when fewer than fifteen existing streams are bulk.
These are component tests, not full protocol/reference/mock conformance.

The 4 MiB limit measures serialized decoded request data, not allocator RSS:
JSON tree overhead, parser temporaries and the separately bounded provisional
frames are additional memory. Incoming and outgoing operations deliberately
share the per-peer total budget; direction is not a way around sixteen streams.
The reserved slot guarantees control admission under **bulk** saturation, not
under saturation by already-admitted control operations. All operation work,
including waiting for unary FIN, remains bounded by the request deadline.

## Remaining before full protocol acceptance

1. Durable atomic one-use enrollment, exact grants/delegations/context scopes,
   expiry/revocation and rate limiting; integrate with the kernel, not a second
   independent policy authority.
2. Integrate scheduler permits with the future multi-frame Git operation
   lifetime; unary tasks and classification are bounded in this component.
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
