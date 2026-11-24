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

## M3 components (not freeze or activation qualification)

PR95 / Accepted ADR0019 establishes the design and component foundation: closed
draft-policy validation, explicit gate artifact authority and a pure probation
reducer. `bind_policy_registry` and
`install_policy` are **trusted-local operator APIs only**, not wire/factory
methods. They retain immutable role/repository metadata and exact deterministic
CBOR revisions in this Store, CAS replacements and atomically audit installation.
Repository bindings are opaque ASCII references (`._:/-` plus alphanumerics),
not URLs. Current target `hub.git.push` enrollment/delegation is an additional
eligibility requirement; it does not authorize the administrative caller.

`promotion_installation` exercises restart/idempotency, role isolation,
credential-URL rejection, injected audit failure and competing SQLite-connection
CAS. Policy replacement invalidates unattempted proposals atomically; activating
work refuses replacement until explicit recovery is available. Existing probation
limits remain pinned. Operator installation errors are returned, not promotion
rejection receipts. Corrupt/deleted store rows fail closed; there is no automatic
policy repair API.

`Kernel::promotion_request` adds **trusted-local** propose/promote admission in
this same Store. The operator authenticates `PromotionCaller` and pins explicit
`PromotionAuthority` and `PromotionBoundary` implementations. No startup flag,
wire method, permissive authority or production adapter is added. Input is bounded
to 64 KiB. Exact request bytes and actor/ticket context are retained permanently
under `(actor, coglet, correlation_id)`; identical retries return the exact old
receipt without new effects or audit. Conflicting bytes/context cannot replace it.

Admission holds the State → boundary fence and an IMMEDIATE SQL transaction,
serializing registry/current-policy/generation reads through commit. Actor authority
uses a revision CAS witness. Shared enrollment/delegation Authority uses a separate
connection to `kernel.db` and is freshly rechecked before commit. Those existing
cross-connection checks are **not** a single transaction-bound authority lease.
Build/signature, required policy+ticket gate union, original-propose approval and
capability/state compatibility are rechecked at promote, including after adapter
preparation. Even optional approval references must authenticate before audit.
Semantic refusal rolls back tentative state via a savepoint, then retains a static
reason-code rejection receipt+audit; storage failure aborts the entire transaction.

Propose does not move live state. Promote reserves a checked higher generation and
atomically retains receipt, audit, state and **unsigned** outbox intent. Outbox
metadata includes a unique common-grammar operation ID, full original propose and
promote contexts, binding, signature reference and device identity. The verified
bundle is a separate BLOB (nonempty, at most 64 MiB), not a CBOR integer array.
The local schema is unreleased; incompatible experimental outbox layouts fail
closed instead of silently rewriting retained commands. Card C must extend this
same outbox, revalidate all authorities, retain exact signed bytes before I/O,
reconcile the original `hub.git.push`/`hub.receipt` scope, and independently attest
applied generation/executable before changing live state. Nothing here signs,
sends, activates code, starts probation or qualifies a native runtime.

Named `promotion_admission` proofs cover:
- `proposal_receipt_is_exact_and_survives_restart_without_new_audit`,
  `same_actor_and_id_on_other_target_does_not_alias_receipt`,
  `independent_sqlite_connections_serialize_competing_and_identical_proposals`,
  `simultaneous_distinct_requests_have_one_pending_proposal`;
- `proposal_faults_leave_no_partial_receipt_or_transition_after_restart`,
  `rejection_receipt_is_exact_after_fault_and_restart`,
  `promote_admits_one_exact_outbox_intent_without_claiming_activation` (including
  receipt/audit/outbox fault injection, exact BLOB bytes and restart);
- `authority_cas_rejection_rolls_back_proposal_but_retains_receipt_and_audit`,
  `policy_replacement_invalidates_unactivated_proposal_and_audit_failure_is_atomic`,
  `proposal_rejects_wrong_base_generation_policy_and_ended_ticket`;
- `ticket_union_approval_request_binding_and_compatibility_are_independent`,
  `artifact_revocation_during_boundary_preparation_rejects_without_outbox`,
  `optional_unverified_approval_is_rejected_without_secret_in_audit`,
  `admitted_audit_links_verified_build_gate_and_ticket_evidence`;
- `absent_boundary_and_generation_overflow_leave_proposal_unactivated`,
  `invalid_boundary_bindings_or_bundle_hash_never_enter_outbox`,
  `incompatible_unreleased_outbox_schema_fails_closed_on_open`.
These fixture authorities prove local storage behavior, not production artifact
verification or native activation qualification.

The reusable `sim/coglet_sim/evaluation.py` runs exact rewritten guest bytes with
real cogletd/WAMR and independent replay, returning behavioral pass/fail. Its M2
fixture provenance and unsigned results are **not** authenticated production gate
artifacts or build authority. Durable promotion/ticket/audit receipts, signed-ref
boundary activation, health/emergency-fence producers, rollback integration,
clock recovery, final CI/review/merge and `policy-v0` are still unfinished.

## Local operator configuration (not deployment qualification)

`serve` now starts the production bounded AF_UNIX Handler at
`<state-dir>/operator.sock`. OS credentials must match the service UID, including
for root; the socket retains the exclusive state lock. Framing, capacity,
permissions and stale-entry refusal use the existing driver. No iroh method,
ALPN or TCP listener is added for administration.

The optional trusted startup `operator` configuration is closed JSON:

```json
{
  "operator": {
    "uid": 1000,
    "actor": "hub:REPLACE_WITH_PROVISIONED_64_HEX_ID",
    "operations": ["grant.provision", "grant.revoke", "enrollment.window", "enrollment.invite"],
    "max_window_ms": 60000,
    "grants": [{
      "peer": "REPLACE_WITH_EXACT_RUNTIME_NODE_ID",
      "method": "hub.status",
      "target": null,
      "max_expires_unix_s": 1800000000
    }]
  }
}
```

Replace the illustrative identity/expiry values with explicit local provisioning.
The UID must equal the actual service UID; the actor uses the common source-ID
syntax. There are no ticket grants or wildcard peers/methods/targets. Each exact
policy ceiling bounds both grant provisioning and initial invite grants; even an
empty invite needs an allowed peer. Absence of `operator` keeps the listener but
refuses **all** admissions; it never derives an actor from the UID. A missing
operation or ceiling denies admission. This producer supports configuration only:
`deploy`, `push`, `stop` and `status` remain refused until the qualified saga
producer supplies their executor. No deployment is claimed.

New installations keep shared Authority tables in `kernel.db`. Any existing
`authority.db` entry fails startup with an explicit offline-migration requirement,
before creating replacement state; there is no automatic migration or deletion.
Configuration, terminal receipt and audit use Store's **exact caller-owned SQL
transaction**, under State → boundary → Authority locks. Shared Authority methods
never independently commit from inside it. A failed effect rolls back before a
semantic failure receipt is retained. Storage/audit failure leaves the admitted
queued receipt available for identical retry, rather than claiming no admission.

Grant revocation retains a strictly higher-generation, expired tombstone. Current
transport polls recheck exact Authority under State → boundary and are woken after
commit; a blocked real-iroh write is reset. Old provision IDs/generations do not
resurrect the grant. Peer grants do not implicitly provision actor/ticket scopes.
The health-generation index changes in the same transaction; incompatible pending
health invitations are refused rather than corrupting the first invite.

Window configuration and hash-only invitations write through that same transaction.
Monotonic liveness is staged under the Authority lock and published only after the
receipt/audit commit. Invites are bounded to their referenced window, one-use and
expected-runtime-bound; no plaintext enters storage or audit. Replacement/close
clears old liveness. Restart never reconstructs it, and retrying an old applied
window/invite returns its retained receipt without reopening it.

Named production-seam proofs in `operator_service` cover socket configuration,
actual lost-reply retry, missing mapping/local-policy denial, hash-only retention,
restart/expiry/close, configuration+receipt+audit rollback and grant tombstones.
`operator_startup_socket_uses_explicit_service_actor` exercises the real binary.
`hardening::operator_socket_revocation_resets_blocked_real_iroh_write` observes a
code-1 stream reset after a socket revoke. The privileged Linux negative is explicit:

```sh
cargo test --locked -p coglet-hub --test operator_peer_process -- --ignored --exact operator_peer_uid_nonowner_subprocess
```

It requires passwordless sudo to launch a genuine root client against an ordinary
service UID, verifies credential rejection despite DAC bypass, and includes the
same request under the owner UID as a positive control. Both privileged test/helper
are ignored in ordinary portable runs; their skipped presence is not this proof.
The remaining ADR deploy saga and runtime/mock lifecycle matrix belong to the
integration producer, not these configuration tests.

## Local fixture controls (not production)

Enable `coglet-hub/test-controls` explicitly in the test launcher's Cargo
configuration; it forwards to `coglet-proto/test-controls`. Defaults remain empty.
`Kernel::test_invite(peer, secret: String, ttl, grants).await` provisions the
supplied 64-lowercase-hex secret through the same Authority storage and kernel
health-generation policy as `invite`. `Kernel::test_close_window().await` closes
the real Authority window. Both use the existing bounded blocking executor.
No startup flag/config field or remote method enables them.

After `open_window`, use `"11".repeat(32)` for the unchanged enrollment fixtures.
For expired invites choose a short positive TTL and wait past expiry before
connecting. For a closed window, provision a live invite then close it. For
already-used fixtures redeem once through enrollment first; for wrong-node
fixtures provision against another actual peer key. Use a fresh isolated state
directory per case. Never translate incoming tokens or provision via SQLite.

```sh
cargo test --locked -p coglet-hub --features test-controls --test test_controls
```

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
  revocation/control retain independent executor capacity. Buffered cancellation
  consumes the landed shared Session reset guard and is tested below.
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

## Hardening and trusted runtime adapter

PR57's shared reset-on-drop guard is now consumed without proto changes. Real
QUIC tests exhaust all 16 incoming bidi credits (blocked open), and constrain
send/stream receive windows to 1 KiB while sending a 200 KiB action (blocked first
write). Tests observe code-1 STOP/reset, not merely cancellation of a Rust task:

- `blocked_open_cannot_send_after_ticket_end_or_caller_abort`
- `blocked_first_write_resets_on_abort_ticket_revoke_deadline_and_clock_loss`
- `ticket_end_during_live_reply_wait_resets_and_survives_restart`

New sends must use `dispatch_action_with_clock`: a **trusted local adapter**
supplies a bounded, nonblocking callback returning fresh logical time in the
receiving runtime epoch of the retained envelope. Return `None` when that epoch
or mapping is no longer verifiable (including receiver restart). No wire clock
sync is invented, no Unix expiry is translated, and no sampled time is persisted
as a lease. The callback must not reenter the Kernel. Missing/lost time and
`now >= mapped_deadline` fail closed, both before attempt marking and immediately
after blocking Authority checks before each transport poll. A pre-I/O refusal
leaves the retained action unattempted. After an ambiguous attempt, only original
scope receipt reconciliation is allowed; this may proceed after the action
expires. `dispatch_action` without a clock is therefore recovery-only, not a
way to send new actions. The receiving runtime still owns final admission;
reset cannot undo bytes/effects already delivered. This is local policy enforcing
`spec/hub-protocol.md` lines 356–372, not an alternative remote authority.

Named deadline/mapping regressions:

- `new_dispatch_requires_live_runtime_clock_before_attempt_marking`
- `deadline_admission_is_exclusive_and_lost_clock_never_marks_attempt`
- `mapping_absence_overflow_and_restart_never_create_or_remap_intent`
- `grant_expiring_during_delegation_check_is_denied`: re-intersect the shared
  Authority grant at the later clock sample owned by delegation verification.
  The two Authority APIs are not an atomic lease; the kernel's poll/control fence
  and receiving-side authorization remain necessary.

The restart/negative matrix includes
`cancellation_audit_failure_rolls_back_ticket_reply_and_live_fence` and
`queued_failed_cancelled_receipt_finalization_survives_restart`. Queued receipt
observations remain nonterminal/reconcilable after reopening; failed/cancelled
receipts are durable and cannot be replaced by a stale queued observation.
`queued_receipt_cannot_mask_committed_ticket_end` retains the deterministic
commit-before-finalization race. Real-network fixtures in the two-device test
binary serialize against one another to avoid unrelated fixture fsync/crypto
contention; each existing 500 ms control assertion is unchanged.

This is still the event-kernel subset and local action-adapter hardening, **not
full action/scheduler/Git/runtime/mock conformance**. Final independent exact-head
review, green CI, merge and this card's final review handoff remain landing gates.

Full scheduler/Git/reference+mock integration stays with the producer-split owners
listed in `crates/coglet-proto/README.md`; do not duplicate their transport state.

## Local initial-deploy coordinator

See [operator-saga.md](operator-saga.md) for protected recipe configuration,
original-child receipt recovery, real-iroh lifecycle qualification and its named
test matrix. Installed-target activation remains outside this initial-deploy slice.

