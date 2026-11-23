# coglet-hub — kernel part 1 (in progress)

Hand-edited kernel built from frozen `spec-v0` and current `coglet-proto` APIs.
No product, mindister or legacy/v0 implementation was copied.

## Implemented and exercised

- Persistent hub iroh identity; `coglet-hub identity STATE_DIR` prints its public
  node ID. `coglet-hub serve STATE_DIR CONFIG.json` serves `coglet/hub/0`.
- `Kernel` delegates one-use invite/enrollment, exact grants and signed hosting
  delegation to one shared `coglet-proto::Authority`. Synchronous SQLite work
  is dispatched through a bounded blocking budget, not the async reactor.
- Local operator registry with exact surface source/event and actor/ticket/action
  policy. No provisioning RPCs are added to the frozen protocol.
- Durable relay cursor/batch receipts and source watermarks. Nonempty duplicate
  batches are checked before watermarks. Queue acknowledgements preserve permanent
  batch receipts; pending payload delivery tokens do not get reused.
- SQLite WAL/FULL append-only audit, transactional with relay/action retention.
  Invites and event payloads do not enter audit records.
- Full once-mapped action requests retained before reverse-stream dispatch;
  repeated IDs cannot acquire a new deadline or different intent. The trusted
  local adapter must explicitly establish the receiving runtime's epoch offset;
  there is deliberately no guessed mapping or new clock-sync wire method.
- Real two-device iroh test: separate invitations/enrollment, one admitted event,
  local test-coglet decision, reverse action delivery to the other mock surface,
  no duplicate dispatch, matching action-outcome relay and audit observations.
- Local `Kernel::status` is an explicit operator-only metadata snapshot, not a
  remotely authorized debug endpoint. It contains no event contents or prompts.

## Build and tests

```sh
cargo fmt -p coglet-hub --check
cargo test --locked -j 2 -p coglet-hub
cargo clippy --locked -j 2 -p coglet-hub --all-targets -- -D warnings
python3 crates/coglet-hub/run_conformance.py
```

The last command drives **10 unchanged `conformance/hub/event-*.json` cases**
through the shared language-neutral harness and a test-only stdio/iroh adapter.
Every request crosses an actual encrypted Session into this Kernel. The adapter
also reopens the durable store and checks admitted event/audit counts before the
last reply. The runner refuses skipped/missing/failed cases. Neither fixture
bytes nor ALPN requests are rewritten. This is the **event-kernel subset only**,
not full hub, scheduler, Git, device-runtime or mock conformance. All fixture
seeds/provisioning are confined to the example, never the production binary.

Startup config is a closed JSON object with `peers` (device, grants) and `coglets`
(Registration) arrays. State is private to the operator. On serve, stdout is an
explicit local bootstrap descriptor containing public connection info and
fresh invitations; **redirect it into a private file, not logs**. Windows default
closed when no peers are configured, otherwise bootstrap enrollment lasts 60s.
Revocation/provisioning are local Kernel APIs. Restart closes old invite windows.

## Recovery and local adapters exercised

- Action attempts reconcile `hub.receipt` in the exact original actor/ticket/target
  scope, never replaying an ambiguous action. Unknown effects have a durable
  `unavailable` result with explicit details; applied/queued receiver receipts are
  returned as receipts, not invented action-accepts. Concurrent live duplicates
  are backpressured rather than racing receipt lookup before remote admission.
- A short synchronous fence covers each transport-future poll. Revocation and
  ticket termination share that fence, wake/cancel pending dispatch, and never
  wait for a peer's network reply. A real-wire test stalls A while dispatching B,
  admits an outcome which overtakes accept, and revokes A within bounded time.
- Permanent exact actor/ticket tombstones cancel retained unapplied deliveries
  transactionally with audit. Already attempted effects cannot be undone.
- Registry provisioning first commits an audited exact intent. Registry/audit
  failure is fail-closed and repaired by an identical retry after restart; malformed
  initial proofs cannot poison an identity. `recover_registration` is an explicit
  local operator API for a strictly higher signed delegation generation when the
  pending proof expires. It cannot change device/policy, audits approval before
  Authority changes, and survives repeated crash/expiry gaps. It is never automatic.
  Authority remains the binding owner.
- Shared Authority and exact actor/ticket policy are rechecked before every
  transport poll on a separate bounded blocking executor (no SQLite on the reactor).
  Expiry/clock loss also wakes stalled dispatch through a bounded 25ms check;
  revocation/control retain independent executor capacity. Buffered stream reset
  still requires the scheduler producer's Session guard (PR57), not this fence alone.
- A late matching authenticated outcome remains admissible after a locally
  persisted unknown-effect observation; uncertainty never becomes a fake runtime
  rejection. Original observation bytes remain stable and outcomes dedupe normally.
- Initial private speech sources are exclusive to one registered/pending coglet;
  another registration/subscription cannot widen owner-only visibility.
- Accepted health uses a hub-local monotonic timestamp, not peer uptime. Status
  degrades after 30 seconds, disconnects on final transport loss, and boots
  disconnected. Tokens distinguish concurrent/replacement connections. Cleanup
  has reserved bounded capacity, so bulk request saturation cannot drop it.
- Trusted local `event_deliveries`/`ack_events` expose stable target-scoped tokens;
  ack is permitted only after the consumer's durable commit. Action outcomes are
  retained/deduped for the identity lifetime, independently of queue consumption.
- The binary now holds an exclusive state lock and publishes a fully fsynced key
  through atomic rename. A subprocess smoke checks serve startup, concurrent-owner
  refusal, SIGINT shutdown, and stable-identity reopening.

## Not ready to land — remaining work

The early independent review and focused WIP review are **not ship approval**.
Remaining same-card acceptance work:

1. Integrate the existing scheduler producer's Session reset-on-drop guard (PR57,
   owner `t_01a2083fe588e786`) after its normal merge gates; do not modify its files
   here. Verify blocked opening/write cancellation against that real transport.
2. Additional hardening: first-write/expiry/revocation races and caller abort,
   scoped ticket ending during an in-flight action, queued/failed/cancelled receipt
   recovery and restart, audit-atomic cancellation, unmapped/overflowed deadlines.
   In particular, a queued receipt racing ticket cancellation needs its final
   local terminal-state recheck before returning. Receiving-runtime action-deadline
   checks are not proven by Unix delegation-expiry tests; do not conflate epochs.
3. Review ambiguous-action receipt semantics against the frozen protocol before
   claiming full action conformance. Current conformance claim remains only the
   10 unchanged event-kernel cases above; no scheduler/Git/runtime/mock claim.
4. Final independent exact-head review, current-main rebase, green CI, merged PR,
   and the card's final review handoff remain required.

Full scheduler/Git/reference+mock integration stays with the producer-split owners
listed in `crates/coglet-proto/README.md`; do not duplicate their transport state.

