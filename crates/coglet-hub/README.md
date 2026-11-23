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

## Not ready to land — remaining work

An early independent source review found integration gaps; it is not approval:

1. Ambiguous action attempts are fenced against blind replay, but must gain
   explicit receipt reconciliation and durable unavailable/unknown-effect results.
2. The current dispatch path holds a global async boundary lock across network
   I/O. Replace it with short admission fencing and revocation-aware cancellation;
   prove stalled peer A cannot delay revocation or peer B.
3. Registration commits Authority's delegation before kernel registry/audit.
   Add an explicit recoverable provisioning transaction/intent so a registry or
   audit failure cannot leave an unrepairable half-registration.
4. Complete health elapsed-time degradation/disconnection observations, ticket
   termination and queued-action cancellation, negative forged-outcome coverage,
   and local adapter exposure of acknowledged queue delivery.
5. Hardening/recovery tests, final independent exact-head review, green CI and
   merged PR are still required. No final acceptance or production-readiness claim.

The initial review's missing action-outcome admission and permanently saturating
pending event store were fixed test-first. The other findings remain open above.
Full scheduler/Git/reference+mock integration stays with the producer-split owners
listed in `crates/coglet-proto/README.md`; do not duplicate their transport state.
