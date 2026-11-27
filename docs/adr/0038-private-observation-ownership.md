# ADR-0038: Private observation ownership and transfer fences

- Status: Proposed
- Date: 2026-11-25
- Authors: Coglet H-private qualification owner
- Related PR: This ADR's accepting PR
- Supersedes / superseded by: Supplements ADR-0023; does not supersede ADR-0031

## Context

[ADR-0023](0023-m1-residual-withheld-routes.md) requires a separate decision
before implementing ownership transfer. The [router protocol](../../spec/hub-protocol.md)
requires owner-only speech relay. The [speech contract](../../spec/capabilities/speech.md)
keeps adapter leases and heard items owner-bound. Enrollment, hosting delegation,
subscriptions and a manage-method grant alone cannot authorize a private audience.

Today `Kernel::register` reserves speech sources exclusively across committed and
pending registrations, and registrations are immutable. `event_deliveries` and
`ack_events` are trusted-local target-scoped APIs, not remote export authority.
The runtime receiver implements bounded authenticated back-channel serving on
hub/0, but has not yet migrated to [ADR-0031](0031-manage-port-and-filesystem-patches.md)'s
hub 1.0 filesystem/manage contract. The 1.0 spec accepts `debug.*` input aliases
until 2027-02-25, normalizing them before validation/authority lookup; removed Git
methods have no alias. The existing hub/0 receiver is not that normalization.
Neither that receiver nor local event proof qualifies
ownership transfer or private read/export. This ADR selects the missing contract;
it does not award qualification or enable a route.

## Decision

### Scope and authority

Select a **trusted-local router operator API**, not a new iroh method, for
transferring one exact adapter source's private observation ownership between two
already registered coglets. No wildcard sources, actor delegation or audience
lists. Transfer does not move a coglet between devices, replace hosting delegation,
install grants, transfer adapter leases, copy private state or replay history to
a new owner. Source-wide ownership is deliberately conservative: all `speech.*`
observations from that source have one owner at this router boundary. This does
not relax the adapter's finer mic/speaker/session/item ownership checks.

The caller is an already authenticated local administrator, using the same
trusted-local boundary as registry configuration. This library API is not exposed
on a socket or network without a separately reviewed administrative binding.
Its implementation must share the existing router Store and audit custody; a
second ownership database or a parallel receiver is not permitted.

### Persistent identity and compare-and-set

Keep immutable `Registration` input and its identical-retry semantics. Add an
**effective private-policy overlay inside the existing Store**, keyed by source,
containing `(owner_coglet, epoch, event_kinds, phase)`. For private events the
overlay, not old `Registration.events`, is authoritative; nonprivate policies
are unchanged. Registration's source-exclusivity check must consult it as well
as pending initial reservations. Retrying an old identical registration cannot
reacquire a transferred source or change the overlay.

Initial ownership is the sole coglet with an exact `speech.*` source entry in
`Registration.events`; it is not a new caller-provided owner field. Adopt that
owner and the exact event-kind set at epoch one in a writer transaction before
any private route is used. Refuse adoption if committed and pending reservations
name different owners, and do not activate a pending-only registration. New
registrations establish this same overlay atomically at final registry admission.
Adoption/final admission and metadata-only audit commit together.

Select local library operations `prepare_private_transfer` and
`commit_private_transfer` on `Kernel`; the caller supplies an explicit
administrator actor for audit and is responsible for authenticating it at the
trusted-local boundary. No software caller obtains that trust from the string.
The prepare input is an exact source, expected old owner, expected epoch, new
owner, a nonempty sorted unique list of exact `speech.*` event kinds, and an
operation ID using [the conventions' correlation-ID grammar](../../spec/conventions.md).
The kinds are a subset of the old effective policy. The old owner loses all
kinds; omitted kinds become unavailable, not still owned by the old coglet.
Both coglets must already be registered, but the new registration need not and
must not already claim the source. The supplied kinds become its effective
private policy through this explicit administrative operation only.

Prepare compares owner and epoch inside an IMMEDIATE writer transaction, locks
both hosting identities/delegation generations and the source, and allocates a
checked higher uint64 epoch. Refuse overflow, another pending transfer, pending
registration/recovery for either target/source, malformed identities, missing
owners and stale CAS without effects. Persist the exact intent, participant
identities and generations, administrator actor and pending decision, and set
phase `fenced`. Revalidate target registrations at commit. Registration/recovery
writers must reject changes to either participating target while this decision
is pending. No registry JSON is rewritten, so unrelated policies cannot be lost.

Operation IDs are unique in the transfer journal. Identical prepare retries
return the retained preparation; conflicting reuse conflicts. Identical commit
retries return the original terminal receipt, never increment or purge again.
A→B→A never reuses an epoch. Preparing, retrying or restarting never activates
the new owner or clears the durable fence.

### Runtime coordination without a new network method

The transfer is a fail-closed administrative saga, not a cross-database atomic
write. The router and the old/new hosting runtimes each retain their own part
in their **existing** durable Store/WAL; there is no second ownership database.
The operator carries signed local configuration artifacts between the existing
router and runtime library APIs. There is no remote self-provisioning method.
The artifacts use deterministic CBOR and domain-separated signatures under the
existing router/runtime node keys, never a coglet's private key. Their closed
fields are exactly: domain/version, router, source, transfer ID, old/new coglet,
old/new hosting runtime and pinned delegation generations, old/new epoch,
event kinds, and artifact phase. Runtime acknowledgments additionally bind the
preparation digest and their own runtime ID; commit certificates additionally
bind the sorted acknowledgment digests. An unknown field/version is invalid.
Signatures authenticate artifacts, not operator intent: a runtime must already
pin that router and both affected target/source bindings in local policy.

1. After the prepare transaction commits, the router signs its retained exact
   preparation. Preparation signing failure leaves the source fenced; retry can
   sign the same bytes. No signing failure rolls back a durable decision.
2. The operator invokes `DeviceHandler::prepare_private_transfer` locally on
   every distinct pinned old/new hosting runtime. This is an explicit new local
   API in the existing receiver, not an alternate server. Each checks the router
   signature, local pins and monotonic source epoch, durably installs a pending
   deny-all private-export binding for this source, cancels/resets old private
   sends and waits for the old publication critical sections to finish before
   signing an acknowledgment. Repeated identical preparation is idempotent;
   different bytes at that epoch conflict. Retain the binding in the existing
   runtime WAL/control state, including tombstones for removed targets; restart
   must never reconstruct permission from static startup config or old grants.
3. `Kernel::commit_private_transfer` verifies an acknowledgment from **every**
   distinct pinned participant, with exact preparation digest, transfer ID,
   source, owner/epoch and delegation generations. Under the same writer
   transaction it rechecks pending decision and unchanged participants, activates
   the new effective policy, purges old undelivered payloads, and retains the
   terminal receipt and metadata-only audit. Then it signs the exact retained
   commit certificate. Signature failure leaves that committed receipt available
   for signing on identical retry, not an ambiguous second commit.
4. The operator invokes `DeviceHandler::commit_private_transfer` locally with
   that router-signed certificate. Only a matching durable preparation can become
   active for the new owner at the new epoch. The old owner remains fenced;
   activation installs no adapter lease, peer grant or actor grant. Missing local
   authority means unavailable. A delayed/duplicate older artifact can never
   lower an epoch, change participants or undo a pending higher epoch.

A missing participant, lost reply, operator interruption or restart can leave
both owners unavailable; resume the **same** retained preparation/commit. There
is no timeout rollback, implicit abort, automatic lease renewal, or automatic
unfencing. If a pinned participant is permanently lost, this mode cannot finish;
it stays fenced rather than treating expiry as proof of revocation. Replacing a
participant or introducing recovery/abort is a separate reviewed contract, not
an implementation shortcut. A reverse transfer is possible only after completion
and uses another higher epoch. This favors privacy over transfer availability.

### Fence and purge boundary

Preparation blocks router admissions, deliveries and private exports for either
owner immediately. It serializes with the existing State→boundary lock order;
no network wait occurs while either is held. Each transport poll obtains a fresh
bounded authority/ownership check and holds the short publication fence only
through that poll; a Pending result releases it. Durable source phase/epoch must
also be rechecked through the Store, so separate Kernel/SQLite connections cannot
bypass a process-local mutex. Runtime export uses the equivalent local durable
binding and per-poll fence before acknowledging preparation. An acknowledgment
means old publication is already fenced, not merely that cancellation was queued.

Commit purges undelivered old-owner private payloads from storage, not just queue
indexes. Preserve permanent relay batch receipts, source sequence watermarks and
correlation/terminal deduplication tombstones. If existing retry retention stores
payload bytes, replace only the revocable private payload component with a retained
digest/tombstone; preserve exact admission decisions and reject payload-conflicting
retries without retaining or redisclosing revoked plaintext. Transfer cannot make
old envelopes admissible again.

Every private admission/delivery captures source, owner, epoch, target and
correlation scope. The existing trusted-local event snapshot APIs are not export
leases: introduce epoch-bound scoped delivery/ack APIs for this mode, and refuse
private payloads through legacy `events`/`event_deliveries`/`ack_events` while the
mode is enabled. An acknowledgment must match the captured owner/source/epoch
and token; a stale token cannot consume new-owner deliveries. Public/nonprivate
behavior remains unchanged. No local helper holding a previously returned value
may publish it without the final current binding check.

Bytes already received before preparation cannot be recalled. Historical runtime
journals and archives remain unchanged for offline replay and are not inherited
by the new owner. Private records must retain their original source/owner/epoch;
if a path/result cannot be classified safely, deny its whole export. An old mixed
archive cannot be exported merely because a target owns some newer source epoch.
Purge of pending payloads is intentionally lossy; completing and reversing a
transfer does not restore them. Local adapter lease acquisition remains a
separate explicit operator action, with all adapter ownership checks intact.

Audit records bind administrator actor, transfer ID, source, participants,
old/new owner and epoch, exact kinds, preparation/acknowledgment digests and
phase, never private payloads, tokens or private keys. Audit/receipt failure
rolls back its local transaction; it cannot unfence a committed earlier phase.

### Private reads and the manage port

Use the existing receiver, authority intersection, path limits and bounded reads;
do not invent a private-read RPC or an alternate serving implementation. In
addition to exact peer/method/target/actor/ticket grants, private observation
access requires an explicit trusted-local binding of authenticated actor to the
current owner and source epoch. Never derive that binding from an asserted actor
string, subscription, enrollment or delegation. A manager has no implicit private
read privilege. The receiving runtime must enforce its own local private export
policy; a router check alone cannot qualify receiver privacy.

Read results, log tails, status/step data, receipt details and error details that
contain private observations share this rule. An unrelated method grant cannot
launder private contents through another response. Read/stream admission binds
owner, epoch, target, actor/ticket and correlation; recheck before every disclosure
and after blocked transport waits. Revoked or stale authority returns a bounded
non-disclosing refusal with no private bytes or ownership/existence details.
Unknown and inaccessible private items must not be distinguished by error payload.
Historical receipt retention is not continuing export authority.

For current main, the normative method names remain ADR-0031 `manage.*`.
Receiver migration is a prerequisite for claiming those wire positives. The
specified temporary `debug.*` alias normalization remains in force; this ADR
does not restore removed Git methods or count hub/0 tests as 1.0 qualification.
Component tests must identify their actual receiver contract. H-private stays
withheld until the current normative paths are exercised. No iroh wire schema
or method changes are selected here; signed **local configuration artifacts**
are explicitly new administrative data, not hidden request fields.

Initial receiver source/owner/epoch-one pins must be explicitly provisioned by
the local administrator and durably retained before the first private export.
An absent or previously fenced/tombstoned binding never falls back to startup
pins. The actor-to-owner grant remains independent and exact. Receiver read
classification derives source/owner/epoch from retained records, not an epoch
asserted in a network request. The local preparation/commit APIs above are how
later epochs reach the receiver; the request context remains `{actor, ? ticket}`.

## Alternatives

- Let registration, subscription or delegation select a second owner: rejected;
  they cannot widen private audience or local leases.
- Add `hub.transfer` or a private observation RPC: rejected; trusted local
  configuration is sufficient, and the receiver already owns read serving.
- Delete/recreate registrations and reset offsets: rejected; loses epoch, source
  watermarks, exact retry receipts and stale-observation fences.
- Rewrite historical archives or automatically migrate leases: rejected; destroys
  replay evidence or grants authority outside the router's control.
- A single in-memory owner lock: rejected; reconnect, restart and independent
  Store connections must retain the same fence.

## Consequences

This is a local administrative contract and implementation authorization only
after independent review and merge. It changes no ALPN, request schema, method
catalog, adapter wire envelope or grant implication. Ownership and epochs must
be durable before the mode can be advertised. Existing fail-closed behavior
remains until qualification. Restoring an older database is not a supported way
to reverse transfer; reversal is another explicit CAS at a higher epoch.

No production code, generated code or golden wire fixture changes in this ADR
PR. The follow-up implementation must map this contract into the router's
existing registry/relay/Store and the receiver's current authority/export seams.
If that needs a wire change, stop for another ADR rather than hiding it in tests.

## Verification

Required Rust CI must discover and execute all following tests with zero skips;
proposed names here are obligations, not existing results:

- `relay_private_owner_only`: owner event positive plus foreign target/source and
  unauthorized private read/response negatives.
- `relay_subscription_not_authority`: subscription-only, enrolled-only and
  delegation-only actors receive no payload or extra lease; authorized control.
- `private_owner_transfer_purges_and_fences`: new-owner positive, old queue/payload
  purge, stale token/epoch/target/correlation negatives, retained source watermark
  and same-ID receipt behavior, and A→B→A old-epoch rejection.
- `private_owner_transfer_reconnect_restart`: persist then independently reopen;
  old authenticated sessions and delayed reads cannot disclose; fresh owner works.
- `private_owner_transfer_cas_and_audit_atomic`: competing independent database
  connections, audit/receipt failure rollback, stale expected owner/epoch, overflow,
  conflicting IDs and pending registration; no partial activation or leaked payload.
- `private_read_owner_only`: actual authenticated receiver path, owner positive,
  foreign owner, subscription-only, revoked, stale epoch and wrong correlation
  negatives, including unchanged historical archives and no inherited old history.
- `private_manage_owner_only`: actual authenticated current manage paths and
  blocked-stream cancellation; private data in read/log/status/step/receipt/error
  results cannot escape through another method or survive ownership/revocation.

Retain existing registry exclusivity and bounded local owner event/ack tests.
Run the same applicable language-neutral wire cases against reference and mock;
component tests are not substitutes for authenticated receiver/transport proof.
Only after these pass in required CI may the implementation PR mark H-private
qualified in the [ledger](../reviews/spec-v0-m1-gates.md), naming exact tests
and limitations. No native/acoustic or cross-device lease-migration claim.

## Approval

Proposed for independent exact-head review and merge-queue landing before any
ownership-transfer implementation. ADR acceptance and implementation qualification
are separate. This record does not claim either has happened.
