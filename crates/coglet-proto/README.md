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
  Responder-side Session streaming dispatch is available through `handle_git`
  and `GitStream` (below); guarded initiator transport uses `push_git`/`fetch_git`.
  Current-authority and durable acceptance integration remains incomplete.
  `quarantine::BundleSink` supplies a private bounded raw bundle file, with
  <=32 KiB writes fsynced off-reactor before returning. The limit is checked
  before writing; I/O failure poisons the file. Drop removes its generated
  quarantine directory; an in-flight worker retains exclusive cleanup custody
  after future cancellation. A cancelled append now permanently discards the
  caller's sink. `BundleSink::sweep_orphans` is a startup operation on the private
  parent: it removes only owned, unlocked directories, never live artifacts or
  symlinks. Per-artifact leases are inherited by validation subprocesses; Drop
  defers cleanup to the sweep if a descendant still retains the lease. This raw
  sink alone does not establish Git validity or code admission.
  `BundleSink::check_envelope` consumes append custody and returns an immutable
  `BundleEnvelope` after exact self-contained v2 header/ref/tip binding, complete
  transfer SHA-256 verification and declared pack-object-count preflight. It reads
  at most 32 KiB at a time off-reactor and rejects extra refs/prerequisites before
  import. The count is untrusted until full pack validation; `BundleEnvelope` is
  NOT a verified Git artifact and cannot authorize ref updates. A real Git-created
  logs bundle is transferred over real iroh and checked in
  `real_iroh_git_bundle_envelope_is_bound_to_signed_ref_and_retains_cleanup_custody`;
  malformed envelope tests include truncated packs, extra refs, prerequisites,
  wrong tip/ref/version/magic, object over-count and hash mismatch, with cleanup.
  Raw data-phase tests exercise a 150000-byte transfer and stalled durable
  consumption over real iroh on the hub ALPN, plus sink failure and source EOF.
  They do not establish push/fetch/restart/replay acceptance.
- `BundleEnvelope::validate(&Validator, Limits)` consumes raw custody and returns
  an immutable `VerifiedBundle`, which retains private bundle/repository custody
  and exposes only the verified metadata. No pathname, ref-store write, working
  tree or activation API is exported. Configure `Validator::new` with the trusted
  **absolute installation path** of the matching `coglet-git-validator` binary;
  never take executable paths from remote inputs. Install this binary with the
  service (`cargo build -p coglet-proto --bin coglet-git-validator`). Validation
  currently requires Linux, `/usr/bin/prlimit`, `/usr/bin/git`, advisory file
  locks and a private durable filesystem with allocation units <=4 KiB; it fails
  closed if the required mechanism is unavailable.
  The helper checks the pack checksum and bounded zlib sizes before retaining
  output, supports OFS/REF deltas (including forward REF bases), charges both
  delta programs and reconstructed bytes, and rejects thin/cyclic packs. Every
  stored object must be reachable from the one requested commit. Unsafe modes,
  path components and attributes/modules/data are rejected throughout history;
  logs commits allow only the `logs/` subtree. Expanded tip-tree multiplicity is
  separately precharged before archive generation to stop tree-DAG bombs.
  The helper writes its own precharged loose objects (never asks Git to import
  attacker-compressed bytes), then runs read-only strict fsck and config-scrubbed
  tar archive hashing. Code refs require a valid core Wasm module and canonical
  shared `hub-build-record` whose rewritten digest binds the exact module.
  This checks provenance structure/output binding, **not** retained build-input
  availability, transform/policy qualification, current authority or promotion.
  Defaults: 256 MiB expanded-pack budget, 100000 objects, delta depth 50,
  256 MiB disk reservations, 30 aggregate CPU seconds and 30 seconds wall time.
  The pack budget includes delta programs/results; expanded archive size has
  its own ceiling of the same value, including conservative tar overhead. Disk
  reservations prepay rounded file blocks and per-entry directory growth;
  conservative reservations can reject before the physical ceiling is reached.
  CPU uses three kernel per-process shares (helper, one fsck, one archive), with
  unused integer remainder; configured totals must be 3..30 seconds. A 1 GiB
  per-process address-space backstop also applies. The parent wall watchdog
  kills the isolated process group on timeout/cancellation, keeps the leader
  unreaped until signalling (no recycled-PGID signalling), and retains cleanup
  custody through worker completion. No extra Git subprocess may be added
  without revisiting this fixed-topology CPU accounting.
  Named tests in `bundle_validator` cover real logs/code/delta bundles, all six
  resource fences, malicious delta/expanded-result/tree-DAG bombs, strict fsck,
  archive mismatch, forbidden paths/modes, Wasm/component/provenance rejection,
  cancellation and private cleanup. The CPU-limit oracle uses an intentionally
  nonterminating trusted test helper, not assumptions about pack-decoding speed;
  the real compression bomb separately requires bounded rejection in debug and
  release builds. Cleanup tests exercise the actual lease-aware orphan sweep
  when kernel descendant teardown defers unlinking. `quarantine_cancel`, `quarantine_sweep` and
  the inherited-lease unit regression cover the sink/recovery boundaries.
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

## Local fixture provisioning (explicit opt-in)

`test-controls` is absent from defaults and production dependency declarations.
Only trusted local test launchers should enable it. It adds
`Authority::test_issue_invite(peer, secret: &str, ttl, grants) -> Result<()>`.
Supply exactly 64 lowercase hex characters (e.g. `"11".repeat(32)`) after
`open_window`. It uses the same grant validation, hash-only storage, peer binding,
one-use transaction, monotonic TTL, rate counters and restart invalidation as
`issue_invite`; production `issue_invite` remains OS-random. Duplicate hashes
are rejected, including consumed or pre-restart secrets. Use fresh isolated
stores per fixture; never revive an old token or translate incoming tokens.

For an expired fixture, insert successfully with a short positive TTL, wait past
it, then run the request. For a closed-window fixture, insert with a live TTL
then call `close_window`. Unknown-token failures are not expiry/window proof;
wire failures deliberately remain indistinguishable `unauthenticated`.
No control bypasses authorization and none is a wire method.

```sh
cargo test --locked -p coglet-proto --features test-controls --test test_controls
# Default-only doctest proves the control API is not available:
cargo test --locked -p coglet-proto --doc
```

## Validated fetch integration (bounded acceptance slice)

`AcceptedRefStore::precheck` checks trusted routing pins, current scoped grants,
delegation and the durable ref watermark in one SQLite snapshot before credit.
It is blocking and conveys no lease: use bounded off-reactor execution, a fresh
trusted clock fenced by the Session deadline, and repeat through `accept` after
full Git validation. It shares the exact pin/comparison logic with commit.

The real-iroh test
`accepted_refs::git_acceptance_session::real_iroh_validated_git_fetch_reopen_statement_revalidation_and_limits`
connects actual Session fetch to a Git-created logs bundle, envelope validation,
bounded fsck/archive validation and durable same-Authority acceptance. It checks
retained bundle bytes, identical-statement full revalidation, separate-connection
reopen visibility, changed-tip skipped-generation observation, corrupted-pack and
expanded-limit rejection/cleanup, stale/equal-different pre-credit rejection,
a newer independent connection committing between validation and local commit,
and revocation after validated custody but before commit. The latter proves that
terminal transport FIN does not replace the commit-time authority check.

`FetchHandler::accept` receives a private-field `ReceivedTransfer` carrying the
Session-validated raw size/SHA-256, separately from signed archive metadata.
The consumer needs no preknown bundle hash. The sender snapshot remains a
trusted test input, not a production per-disclosure authorization service.

`accepted_refs::git_acceptance_session::git_push_acceptance::real_iroh_validated_code_push_provenance_replay_and_post_fin_slots`
sends Git-created code bundles through Session push, fsynced quarantine,
fsck/archive/Wasm/provenance validation and same-Authority authoritative CAS.
Invalid Wasm and missing provenance leave no audit or quarantine. Two paused
validations after end+FIN retain both caller slots; a raw third peer stream
bypasses caller admission and independently proves responder saturation. One
concurrent CAS wins, exact bundle bytes are retained, and a new-operation signed
replay is rejected without polling an instrumented source. Success is queued
admission, not live activation or qualified build reproduction.

`PushOperation::new(authenticated_peer, &actual_request)` binds identity,
operation ID and target to a deterministic-CBOR SHA-256 of method, params and
actor/ticket context; timeout is excluded. `precheck_push` permits a known exact
intent without replaying its old-tip CAS, but repeats current authorization.
`accept_push` additionally binds the validated raw bundle's size/hash and commits
its queued receipt in the **same** transaction as ref and retained audit bytes.
Receipt insertion failure or final-check expiry rolls all three back. A duplicate
still completes transfer/FIN, fsck/archive validation and current authority checks;
changed intent conflicts before credit and a new-ID signed replay still fails CAS.
`push_receipt` recovers this exact admission receipt under current authority.
These synchronous calls require bounded off-reactor execution and a fresh clock.
The receipt table currently owns immutable **queued admission only**, not the
router/runtime's later applied/failed/cancelled lifecycle or a `hub.receipt` server.

`accepted_refs::git_acceptance_session::git_push_acceptance::real_iroh_code_push_process_restart_replays_original_receipt`
launches two separate child processes. The first performs actual validated code
push and exits without destructors; the second opens the same Authority database,
recovers the original receipt and replays the same operation through fresh real
Session/iroh endpoints with full validation, returning the exact original reply
without another audit. This proves after-commit process restart, not power-loss
simulation or a pre-commit Session crash. The store-level before/after-COMMIT
process tests remain separate. `git_operation_store` also checks atomic receipt
failure, final expiry rollback, exact context binding and revoked retry denial.

`accepted_refs::git_acceptance_session::git_push_acceptance::real_iroh_validated_published_code_fetch_checks_wasm_and_provenance`
fetches an actual coglet-key-signed published code bundle through Session and
validates Wasm/provenance before durable observation. Validly signed invalid
Wasm or missing provenance leaves no audit; identical valid observations fully
revalidate without a second audit. Source snapshots remain trusted fixture data.

Remaining adversarial streaming cases still need end-to-end qualification;
kernel policy, activation and per-disclosure authorization remain their owners'
responsibility.

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

## Durable accepted-ref component

`authority::accepted_refs::AcceptedRefStore::new(authority.clone())` uses the
Authority's **same connection/database**, including in-memory databases. It is
not a second policy authority and does not wire Session or activate live code.
Only `validator::VerifiedBundle` can enter `accept`; the store consumes its
private custody and atomically retains the complete signed statement, uint64
watermark, tracked tip and append-only acceptance audit (with exact raw bundle
bytes for recovery). No external Git ref or checkout is changed. Recovery bytes
from `audit_bundle` are untrusted data until revalidated, not an activation handle.

Supply `Acceptance` from trusted routing/authentication: pinned hub, coglet and
ref kind, authenticated peer, exact actor/ticket scope, grant generation and
separate delegation generation. The store rechecks the exact enrollment/scoped
grant, ticket end marker, current signed device delegation/method/expiry (or the
pinned hub peer for hub refs) in the same `BEGIN IMMEDIATE` transaction as the
ref CAS. Revocation/replacement on another Authority connection serializes with
that commit. Supply a fresh trusted Unix clock callback; no clock is denial. It
is sampled after lock acquisition and immediately before commit. Callbacks must
not reenter Authority; run this blocking API on bounded blocking workers.

`Mode::Authoritative` requires `hub.git.push`, exact signed old tip and strictly
higher generation. `Mode::Observation` requires `hub.git.fetch`, permits skipped
generations without matching the observer's old tip, and treats only an identical
equal-generation statement as a no-op with its original audit ID. Even a no-op
requires a newly validated transfer and current authority. A scope cannot switch
modes. Use `current` to check metadata before credit; acceptance always repeats
the comparison against the latest durable value. There is no delete/reset API to
erase replay state. Generations are stored as text to preserve all uint64 values.

Named tests in `tests/accepted_refs.rs` cover insert/update fault rollback,
authoritative wrong-old/generation/replay refusal, close/reopen recovery,
subprocess exits before/after COMMIT, concurrent authoritative writers,
out-of-order observer completions, skipped/identical/equal-different/older
observations, real changed tips, retained historical bundles, routing pins,
peer/ticket/delegation revocation after validation and final-check expiry.
The ignored child fixture is executed twice by the ordinary process-restart test.

```sh
cargo test --locked -j 2 -p coglet-proto --test accepted_refs
```

This is component acceptance, not full push/fetch or conformance acceptance.
Session request/receipt operation-ID deduplication (distinct from signed-statement
replay), streaming integration, build qualification/policy and boundary activation
remain with the streaming/kernel integration owners. A replayed authoritative
statement on a new operation conflicts; an exact duplicate **operation** must be
intercepted by its original durable operation receipt before calling this API.

## Streaming responder transport (integration in progress)

`OperationHandler::handle_git` is dispatched for both Git methods without waiting
for unary request FIN: push needs credit before sending data; fetch needs its
request direction open for credit. The default rejects with permission_denied
without invoking the legacy unary handler. Explicit implementations receive a
restricted `GitStream`, not an escapable raw iroh connection/stream pair.

`GitStream::receive_push` binds already-verified metadata to the exact request,
router peer and target, writes to fsync-backed private `BundleSink`, checks the
transfer SHA-256 and requires end + FIN before returning raw custody. The handler
must authorize and precheck CAS **before** calling (the helper issues credit),
then perform envelope/Git validation and durable acceptance before returning its
receipt. `send_fetch` pins metadata to the requested ref/target/router, sends
metadata, reads source bytes only after credit, and requires initiator FIN after
end before success. The sender must separately authorize snapshot disclosure.
These helpers are one-shot; a failed/cancelled transfer cannot resume.

The Session holds the shared two-Git admission permit, operation deadline and
code-1 reset guard over the entire handler future, including validation/commit
and terminal transmission. Errors can reject without consuming the input; unread
input is stopped with code 1. A handler cannot return a successful terminal frame
without completing its streaming helper. This transport gate does not prevent a
custom handler from violating its own authority/transaction contract; never
perform mutations before verified input or bypass the durable store checks.

`Session::push_git` keeps the request direction open until credited chunks/end
finish, then sends FIN and validates the terminal frame/FIN. A valid rejection
before credit is preserved without reading the source. As with `request`, its
`Ok(Value)` can be a wire error: inspect `type`; it is not an activation claim.
The sender trusts its local request/source, reads exactly declared size (not EOF),
and relies on the receiver for hash/admission verification.

`Session::fetch_git` validates transfer schema, correlation and signed routing
pins, then awaits `FetchHandler::authorize` before creating quarantine or emitting
credit. That callback must check current authority and the latest durable
watermark. Terminal reply/FIN precede `FetchHandler::accept`; acceptance must
validate actual Git and recheck authority/CAS in durable storage, even for an
identical statement. `FetchResult::Rejected` distinguishes a remote error from a
local `Accepted` callback result. Both callbacks retain the shared Git permit and
original deadline. A detached blocking commit must use a fresh deadline-aware
trusted clock under the write lock; cancelling its awaiter cannot stop the worker,
and the Session permit is released at cancellation. Receipt recovery is mandatory
when commit outcome is ambiguous. `request` explicitly rejects Git methods locally.

`tests/git_session_iroh.rs` exercises 150000-byte push/fetch through actual Session
initiator and responder APIs over real iroh, push success withheld until FIN,
missing-FIN timeout, trailing-end rejection with quarantine cleanup and code-1
reset, and fail-closed legacy dispatch (mutation counter stays zero). It also
holds two local acceptance callbacks after terminal FIN to prove both Git slots
remain occupied, checks deadline cancellation removes custody/releases the slot,
and preserves a pre-credit rejection with an empty source. These are **raw
transport** tests, not real Git admission. Concrete pre-credit authority/watermark
checks, durable operation receipts, full validator/store integration and
end-to-end restart/replay/limit proofs remain.

## Remaining before full protocol acceptance

1. Durable atomic one-use enrollment, exact grants/delegations/context scopes,
   expiry/revocation and rate limiting; integrate with the kernel, not a second
   independent policy authority.
2. Integrate scheduler permits with the future multi-frame Git operation
   lifetime; unary tasks and classification are bounded in this component.
3. Git push/fetch multi-frame operation integration (the unary API does not
   implement git transfers), sender credit validation, signed metadata/watermark
   checks before disclosure/credit, integration of the isolated bounded validator,
   durable CAS/watermarks and boundary activation. Validator component delivery
   is not Session/end-to-end acceptance.
4. Reference hub/device test endpoints and `coglet-mock::hub`, lifecycle/debug/
   inbox/action/relay behavior with durable receipts and stateful observations.
5. Same `conformance/hub/` cases/oracles against reference and mock using the
   harness's iroh-driver. The current upstream driver is dial-only; the hub suite
   README explicitly requires accept/reverse-stream, reverse-fetch mapping and
   post-run durable oracles before advertising the corresponding fixtures.
   Missing oracle coverage must remain SKIP/incomplete, never PASS.
6. Independent exact-head review, green CI, merge and board review handoff.
