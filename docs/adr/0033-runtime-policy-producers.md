# ADR-0033: Runtime policy attestations and emergency output custody

- Status: Proposed; implementation qualification separate
- Date: 2026-11-25
- Authors: Coglet runtime maintainers
- Related PR: recorded in the publishing PR; owner t_01a210d811a75260
- Supersedes / superseded by: none; supplies ADR-0019's missing local producers

## Context

[ADR-0019](0019-m3-policy-freeze.md#adapter-qualification-and-recovery-obligations)
requires applied-generation attestation, authenticated health, and emergency output
revocation. Frozen hub status and health cannot establish them. An A→B→A commit
sequence particularly cannot establish the applied signed generation. The current
runtime already retains installed pushes, their exact operation intent and scope,
and replacement code in its exclusively owned WAL. A second producer database
would split that atomic boundary.

[ADR-0027](0027-daemon-restart-resume.md) resumes valid running intent after boot;
that must not resurrect a revoked output lease. Router audit/Authority SQL failure
is precisely when emergency revocation must remain possible. Neither stopping a
scheduler eventually nor returning an ordinary status snapshot is an output fence.

## Decision

### 1. Trust boundary and interface

Add a trusted-local producer interface owned by `cogletd`/`DeviceHandler`, not by
the policy consumer. It is opt-in, configured by the device operator. No new hub,
adapter or guest method, grant, or frozen message field is introduced. Existing
implementation identifiers remain until the ADR-0031 migration; a future filesystem
patch carries the same provenance and generation obligations without Git refs.

The interface exposes `attest_applied`, `observe_health`, `enable_output_lease`,
`revoke_output_lease`, and `reconcile_emergency_stop`. These are local operations,
not remotely provisionable authority. The daemon may serve them on a separate
private Unix socket: an owner-only directory/socket, configured peer UID checked
with OS peer credentials, and a finite configured principal→actor/target/ticket
binding. A UID is local operator authority, not proof of a remote actor. Reject an
unconfigured principal or a request inconsistent with its binding before disclosure
or effect; never take an arbitrary request actor as authentication. No TCP listener,
secret in an audit, or public signing service is authorized by this ADR.

Requests and replies are bounded closed JSON objects, one newline-delimited frame
per request, at most 64 KiB each. Require version `1`, method, request nonce and the
configured binding; reject unknown fields, duplicates, malformed frames and excess
size. Read replies echo the nonce and identity. A production consumer accepts only
replies obtained directly from its configured authenticated local connection, not
caller-supplied JSON records or a producer flag. Copied/forged records cannot update
probation. The implementation must publish exact local request/reply schemas and
shared positive/negative vectors before exposing the listener. No claim of frozen
hub conformance follows from local interface qualification.

### 2. Applied provenance is one WAL effect

Retain a versioned `AppliedGeneration` record in the existing managed WAL state,
in the SAME transaction as the successful initial install or installed code cut
and its applied receipt. It contains:

- target and configured router identity;
- the entire verified signed generation statement (including signature), applied
  generation, commit/revision and digest of the active `bin/step.wasm` bytes;
- original code admission operation ID, exact retained intent digest, actor,
  target, ticket (including absence), and method; for initial install also retain
  both the original signed code admission and the install operation/context;
- serialized boundary/trace identity and applied receipt identity.

Never synthesize this from the latest accepted ref, current reader, staged tip,
current start, or commit equality. Reads take the runtime's serialized boundary;
recovery cross-checks the record against the retained signed admission, immutable
code history, receipt scope and actual executable bytes before disclosure. Failed
and queued changes never advance it. A stopped initial install can attest code
installation without claiming a running scheduler or a completed guest step.
Missing legacy provenance reports unavailable; it is not invented on upgrade.

### 3. Health is producer-owned and generation-bound

`observe_health` requires an exact expected target/generation/commit and echoes the
applied operation and executable digest. A mismatch refuses rather than returning a
healthy sample for another generation. At one serialized observation return:

- cumulative attempted, committed, failed and budget-exceeded step counters since
  this activation, updated from actual native attempt/outcome boundaries; unknown
  guest outcomes retain uncertainty, never count as success;
- current admitted capabilities, integrity/attempt-fence/storage state, lifecycle
  state and output-lease state (unknown or unavailable is never healthy);
- ordered observation sequence, producer instance identity, and explicit clock
  mapping: supervisor boot identity/continuous milliseconds and runtime anchor
  identity/logical milliseconds sampled at this boundary.

Counters and activation identity commit in the existing WAL with their relevant
attempt/result boundaries and never regress across reopen. Generation change
creates a distinct counter domain, even when code returns to the same commit.
Observation sequence plus producer instance prevents duplicate/reordered samples;
consumer admission also checks the request nonce and exact expected activation.
Use the existing supervised continuous clock including downtime, not wall time or
transport `Instant`. Missing/changed/unverifiable mappings are unavailable and
cannot complete probation. A logical guest clock is not supervisor time. Overflow
of any counter or ordering value fails closed instead of wrapping.

### 4. Output custody is independent of router SQL

When this producer profile is enabled, each target's output lease starts CLOSED
at every daemon process start, before any adapter listener or dispatch is exposed.
ADR-0027 may restore scheduling only under its existing revalidation rules; it
cannot grant output permission. `enable_output_lease` is an explicit authenticated
local operation bound to the current applied generation, executable and daemon
instance. It first durably retains a preconfigured emergency stop obligation with
its exact original actor/target/ticket/operation ID/intent and authority pins in
the SAME WAL. It must freshly validate authority and integrity before granting.
A historical start retry or new adapter connection cannot enable the lease.

`revoke_output_lease` synchronously closes the local gate before returning, without
acquiring router SQL or waiting for a guest, audit, network call or stop receipt.
All output publication paths (including pending/retried dispatch) share that gate
and recheck immediately before exposure. Revocation linearizes against dispatch:
an output exposed before the revoke may already have a physical effect; none may
be newly exposed after it returns. This does not claim cancellation of a physical
effect already accepted by an adapter. Execution/observation and output authority
are distinct; discarding native actions after sending them is not revocation.

After closure, persist revocation and the stop-pending state in the original WAL.
If WAL persistence is unavailable, report fenced/uncertain, keep the process gate
closed, and prohibit re-enable in that daemon instance. This is NOT a stopped
receipt or a successful durable write. A crash remains safe because all leases
start closed, and the pre-existing emergency obligation must be reconciled before
any new lease is admitted. No second store or best-effort callback supplies safety.
Lease revocation must not require the runtime mutex held during a long guest call;
publication and revoke use separate bounded local custody.

### 5. Reconcile stop in the retained original context

`reconcile_emergency_stop` uses only the exact obligation retained before lease
enablement. It may wait on ordinary current receiving-side authority and WAL
custody, but never holds the output gate open while doing so. Read/retry the same
original `hub.stop` operation and validate its exact intent/context; do not borrow
the reconnecting reader's actor/ticket or allocate a fresh ID on restart. A conflict,
expired/revoked authority, unresolved guest attempt, unknown receipt, or unavailable
storage keeps fenced/uncertain. Only the authoritative applied stop receipt permits
reporting stopped. Preserve receipt nondisclosure and existing ID collision rules.

Restart recovery reconciles the retained obligation before lease admission, never
performs a guest step for reconciliation, and never erases historical receipts.
A fresh independently authorized activation/lease may proceed only after resolved
custody and all ordinary checks, not by clearing a pending flag. The lease stores
no policy threshold or rollback decision: promotion, probation and restoration
remain the consumer's responsibility.

## Alternatives

- Infer activation from accepted refs/status: fails the A→B→A and staging cases.
- Reuse frozen health: lacks cumulative counters, integrity and generation pins.
- Stop via router SQL first: cannot fence outputs when that database is failing.
- Persist a second emergency database: loses the installed-push atomic boundary.
- Restore the last lease on boot: can resurrect an unpersisted emergency revoke.
- Add remote hub methods now: widens the frozen protocol and needs its own wire ADR.

## Consequences

The opt-in producer profile requires explicit output lease admission after every
restart. Legacy installations without the profile retain their existing behavior;
they cannot qualify ADR-0019 policy activation/health/emergency safety. Producer
migration never treats missing WAL fields as authority. Boot-closed output custody
is stronger than merely replaying a durable revoked bit and covers failed revoke
writes without inventing an audit. Unix local authentication is not remote network
qualification. Later transport/schema evolution requires its own reviewed change.

## Verification

Implementation must add named integration tests, each using real `cogletd`, real
WAMR guest execution and a real exclusively owned WAL reopen, not fixture adapters:

- `attestation_matches_applied_generation_after_restart`: initial install, installed
  change, A→B→A, original contexts, executable bytes and no queued advancement.
- `health_counters_bound_to_generation_and_reject_stale`: actual successful/failed
  and budget-exceeded guest attempts, durable counters, wrong generation/commit,
  reordered observations, changed clock mapping and counter overflow controls.
- `emergency_output_revoke_is_synchronous_without_router`: hold/unavailable router
  SQL and a bounded guest call, revoke through the real local endpoint; prove no
  subsequent output exposure, with an authorized pre-revoke output control.
- `emergency_stop_reconciles_original_context_after_restart`: revoke, SIGKILL and
  restart, same original stop ID/scope and authoritative receipt, no implicit lease
  enable; include a failed revoke WAL write and held pending-stop authority.

Negative controls must actually exercise the receiving producer: forged records,
wrong UID/principal, wrong actor/ticket, stale generation, changed executable,
operation-ID collision and unknown stop outcomes. Closed-frame schema vectors and
a black-box local-interface harness are shared by producer and consumer. Retain
existing lifecycle, installed-push, restart, ABI and both adapter transport suites.
Required exact-head CI must execute these native tests rather than filter to zero.
Docs acceptance alone does not qualify any test or unblock the M3 freeze.

## Approval

Independent exact-head review, required CI and the accepting merge-queue PR must
be recorded in the publishing PR before implementation starts. No producer code,
public wire change or M3 acceptance is delivered by this docs-only decision.
