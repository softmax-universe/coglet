# ADR-0043: Reserved local emergency admission

- Status: Proposed; implementation qualification separate
- Date: 2026-11-26
- Authors: Coglet runtime maintainers
- Related PR: recorded in the publishing PR; owner t_01a210d811a75260
- Supersedes / superseded by: refines ADR-0033 local transport admission only

## Context

[ADR-0019](0019-m3-policy-freeze.md) and
[ADR-0033](0033-runtime-policy-producers.md#4-output-custody-is-independent-of-router-sql)
require synchronous emergency output revocation independent of router SQL and
ordinary runtime custody. The implemented producer listener limits all methods to
16 concurrent connections. Idle ordinary clients or ordinary operations blocked
on SQL can consume all slots: a subsequent revoke connection is dropped before
method dispatch. Per-connection threads do not reserve emergency admission.

The local interface document's statement that its shared 16-slot pool prevents
revocation from queueing behind other requests is therefore insufficient. Merely
raising that shared limit preserves the reachable failure. This decision changes
local transport discovery/admission, not the frozen hub, guest or adapter wire.

## Decision

### Two bounded, independently admitted listeners

With the opt-in producer profile enabled, bind BOTH the existing Unix stream
socket at `profile.socket` and a revoke-only sibling whose path is formed by
appending the literal ASCII suffix `.revoke` to the complete socket path (not by
replacing its extension). No new profile field or request version is introduced.
For `/run/cogletd/p/producer.sock`, the sibling is
`/run/cogletd/p/producer.sock.revoke`.

Both use the existing owner-only parent directory (0700), socket mode 0600,
kernel peer-UID authentication and exact configured
principal/actor/target/ticket binding. Preserve live-listener refusal and
same-owner stale-socket recovery. Validate/bind both before exposing any adapter
or admitting any output lease. Failure of either bind fails startup closed and
cleans up only sockets created by that startup, never a live or foreign socket.
Preflight both paths before unlinking or binding either. Start accept workers only
after both binds succeed; an accept-worker spawn failure also fails startup closed.
A path too long for either platform's Unix socket bound is a startup failure,
not permission to omit the emergency listener or silently truncate the path.
The absent-profile legacy mode still creates neither listener.

The ordinary listener retains its 16-connection bound and all five methods.
The emergency listener has its OWN accept loop and independently reserved bound
of four connections, with no shared connection counter, worker pool or execution
queue with ordinary requests. Authenticate the kernel UID before assigning a
worker. A full emergency pool closes newly accepted emergency connections; it
never falls back to the ordinary pool. Both pools remain bounded, including on
thread-spawn failure and connection error. This is resource isolation from
ordinary load, not a claim of unlimited availability under hostile emergency
traffic or process/OS resource exhaustion.

### Revoke-only execution

The emergency socket accepts exactly one existing version-1
`revoke_output_lease` request per connection and closes after its reply. Use the
same closed JSON parser, duplicate-key rejection, 65536-byte frame bound,
nonce/identity echoes, reply schema and exact binding authentication as the
ordinary socket. The complete request must arrive within three seconds of
acceptance (no separate ten-second idle allowance); the reply write is bounded
by three seconds. Close immediately after the single reply attempt, with no
ordinary-lane lingering read/drain. These are local transport resource bounds,
not changes to guest clocks, production step budgets or policy deadlines. They
do not assert a wall-clock upper bound on OS scheduling or the existing fence
operation; neither may be hidden by a successful response before gate closure.

An authenticated well-formed request naming any of the other four methods gets
`invalid_argument` with its nonce and closes, WITHOUT invoking `LocalOps::call`
or acquiring SQL/runtime custody. Malformed or unauthenticated requests retain
the ordinary undisclosing error semantics and close. There is no
`reconcile_emergency_stop`, health, enable or attestation execution on this lane.
Shared vectors must cover valid revoke, forbidden methods, forged bindings,
malformed/duplicate frames and size/deadline bounds.

Valid revoke uses the SAME output gate and original prearmed emergency
obligation as the ordinary method. Its admission, authentication, dispatch and
synchronous physical-output fence do not acquire router SQL or runtime/guest
custody. Gate closure still precedes the response. Preserve ADR-0033's
fenced/uncertain result when durable follow-up cannot finish; do not turn the
reserved worker into a synchronous stop/WAL-reconciliation worker. Stop
reconciliation stays on the ordinary lane against the original retained
actor/target/ticket/operation ID and existing WAL. No new store, stop ID or
authority source is introduced. ADR-0027 boot-closed custody remains unchanged.
Only output-gate closure has reserved capacity: ordinary stop reconciliation may
remain blocked or starved and must not be reported as applied on that basis.

Bound deferred revoke recording as well as front-door admission: retain at most
ONE pending recorder per configured target in this daemon instance, shared by
both listeners. Repeated revokes while that recorder is queued or running report
`pending` without spawning more workers. Reserve its slot atomically before spawn;
release it after completion/error, and on spawn failure mark the gate uncertain
and release the slot. No retry may reopen the gate. A later recorder may retry
persistence, but uncertainty is never silently cleared into enable authority.
The bound is the finite configured target set, not request rate. Preserve the
prearmed WAL obligation and boot-closed recovery; this is in-memory work
coalescing, not a new durable ledger or a claim that pending work completed.

### Consumer migration

Consumers requiring the ADR-0019 emergency guarantee MUST send revocation to the
`.revoke` sibling and authenticate the connection as before. They must not fall
back to the ordinary listener and claim independent admission when the emergency
listener is absent or unavailable. Ordinary-socket revoke stays compatible and
retains its gate semantics when admitted, but has no ordinary-saturation liveness
guarantee. An old producer without the sibling is not qualified for the new
admission requirement. Publish this discovery rule and the method restriction in
`crates/coglet-runtime/policy-local-interface.md` with the implementation; downstream
M3 policy/rollback integration is separate and remains consumer-owned.

## Alternatives

- Increase the shared cap: ordinary clients can still exhaust every slot.
- Inspect the method synchronously in the ordinary accept loop: idle or partial
  frames can block classification before the emergency method is known.
- An unbounded bypass thread per connection: abandons bounded resource admission.
- Route stop through the router first: retains the SQL dependency the emergency
  fence must avoid.
- A new remote method or persistent emergency database: unnecessary authority or
  durability expansion; neither repairs local admission isolation.

## Consequences

Two owner-only socket paths are now required for an enabled producer profile.
Existing profiles and ordinary frames remain valid, but consumers must select the
new path for reserved admission. Misuse or saturation of the emergency endpoint
itself is bounded and may be refused; a compromised configured local principal
can already revoke output and is not converted into a remote security principal.
This decision does not waive resource limits, allow forged bindings, make an
uncertain stop applied, or claim cancellation of already exposed physical acts.

## Verification

Implementation acceptance requires real `cogletd`, real WAMR output and a real
WAL reopen, not just mock `LocalOps` calls:

- `emergency_output_revoke_survives_ordinary_admission_saturation`: establish actual
  guest-generated adapter output, keep all 16 ordinary slots occupied (separate
  idle/partial-frame and SQL-blocked authenticated-operation cases), prove the
  occupancy without timing guesses, then authorize revoke on `.revoke`. Require
  a fenced response while ordinary operations remain blocked and unchanged
  adapter receive-queue bytes after return; release blockers only afterward.
  SIGKILL/restart and verify boot-closed output and the retained original stop.
- Preserve `emergency_output_revoke_is_synchronous_without_router` and
  `emergency_stop_reconciles_original_context_after_restart`; exercise the
  reserved production endpoint, original context, unavailable persistence and
  wrong-actor refusal. Immediately after wrong-actor reconciliation, assert the
  original stop receipt is not applied before performing authorized recovery.
- `reserved_emergency_endpoint_rejects_non_revoke_and_forged_bindings`: prove no
  ordinary operation is dispatched, no lease is enabled and a wrong actor cannot
  close an enabled gate; include partial/oversize frames, exhausted emergency
  capacity, timeout slot reclamation and subsequent valid revoke.
- `repeated_revoke_coalesces_pending_recorder`: hold runtime custody, issue repeated
  authenticated revokes through both listeners, and assert at most one pending
  recorder for the target. Release custody and verify original WAL stop-pending
  state, recorder-slot reclamation, spawn-failure uncertainty and no implicit
  enable. Keep any deterministic unit/fault seam distinct from native coverage.
- `producer_listener_pair_fails_closed_and_reopens`: cover live/foreign paths,
  suffix-length overflow, second-bind failure cleanup, real stale sockets after
  SIGKILL and absent-profile compatibility.

Retain exact original generation-attestation and health acceptance. In
`health_counters_bound_to_generation_and_reject_stale`, actually apply a successor
signed generation with different executable bytes, reject the superseded pin,
prove the new activation's independent counters and exact quiescent WAL-reopen
counts. A never-applied generation alone is not the superseded-generation oracle.

Unit/parser controls supplement but do not replace real-daemon saturation and
restart proofs. Required exact-head CI and independent implementation review must
run/check the named tests. No claim of arbitrary emergency-traffic DoS resistance
or downstream M3 policy acceptance follows from this docs-only decision.

## Approval

Record independent exact-head review, required CI and actual docs-only merge in
the publishing PR before implementation. This ADR alone does not fix the listener
or close the implementation card's review findings.
