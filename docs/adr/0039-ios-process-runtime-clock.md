# ADR-0039: Fail-closed iOS process-scoped runtime clock

- Status: Accepted on merge (implementation remains gated until merge)
- Date: 2026-11-26
- Authors: Hermes
- Related PR: pending
- Supplements: [ADR-0022](0022-embedded-product-catalogs.md), [ADR-0037](0037-babybear-ios-command-disposition.md)

## Context

The physical Baby Bear iPhone qualification of the interpreter-only
`ios-runtime-v0.0.0-rc1` release reached a genuine embedded startup refusal:
`coglet runtime startup refused: kernel boot-session UUID unavailable`.
The application was foreground with protected data available. Its native
`system.status` completed, while the isolated shadow mapping became UNKNOWN.
The runtime WAL contains only its initial snapshot: no clock anchor, guest
attempt, dispatch or committed guest intent. Simulator qualification did not
exercise the physical application's sandbox restriction.

`crates/coglet-runtime/src/clock.rs` currently requires
`sysctlbyname("kern.bootsessionuuid")` on both macOS and iOS. A release that
links for iPhoneOS is not evidence that this kernel identity is accessible
inside an installed iOS application. No permission or entitlement expansion
is proposed. The [adapter protocol](../../spec/surface-protocol.md) recovery,
clock rollback, grants and suspension rules remain normative.

## Decision

Use a deliberately more restrictive continuity profile **only on iOS**:

1. Generate one process-scoped epoch from the OS cryptographic random source,
   with race-safe one-time initialization. Namespace it distinctly from kernel
   boot identities. Entropy failure refuses startup; never substitute a PID,
   civil timestamp, constant, caller-supplied or persisted identity.
2. Measure elapsed time with checked `mach_continuous_time` conversion, including
   elapsed suspension. Retain current rollback and overflow refusal.
3. Persist that epoch in the existing durable clock anchor. Reopening an
   **anchored** WAL in another process refuses before native engine admission
   or service publication, even when the device has not rebooted. Do not
   replace an old anchor, renew grants or retranslate retained deadlines.
4. A genuinely unanchored initial WAL with no dispatch may acquire its first
   anchor under the existing rules. This does not release a product UNKNOWN
   command or authorize replay. It is not general cross-process recovery.
5. Keep macOS/Linux kernel boot-identity recovery unchanged. Do not reuse this
   profile for the router/hub policy clock or silently fall back to it on macOS.

This changes the iOS host continuity implementation, not wire schemas, C ABI
signatures, guest contracts, compute budgets or the interpreter-only policy.
It sacrifices restart availability rather than weaken expiry safety.

The embedded adapter still closes/revokes on suspension. Continuous elapsed
time neither preserves grants nor authorizes background execution. Product
`PodHost`/`CommandJournal` and Coglet mapping journals remain authoritative:
claimed, running and UNKNOWN identities are never recreated, re-admitted or
replayed. A new command's isolated runtime WAL must never be used to retry an
old uncertain command under a replacement ID. Historical WALs stay retained.

## Alternatives

- **Kernel identity or new entitlement:** the real app cannot read the existing
  kernel identity; expanding sandbox privileges is outside this qualification.
- **Boot time, civil clock, PID or stored UUID:** these can move, repeat, or
  survive reboot without proving monotonic continuity. Reject them.
- **Refuse all iOS runtime execution:** remains the safe behavior until this
  design, implementation, new release and product pin have passed their gates.
- **Replace or delete an incompatible WAL:** destroys uncertainty evidence and
  can renew expired authority. Forbidden.

## Consequences and rollout

An iOS process restart can retain/read historical product receipts but cannot
resume an anchored embedded runtime session. Same-process reopen can retain
its original epoch and elapsed time. Neither property proves guest execution
or physical qualification by itself.

Do not overwrite the published rc1 tag or assets. Publish a new immutable
runtime release after independent exact-head review and required CI, then
update the Baby Bear pin through its merge train. Native remains the default;
only the existing `system.status` slice is eligible. The other 50 inventoried
native commands remain native. No new actuator or shadow owner is introduced.
Selected-default cutover remains gated on actual physical lifecycle, protected
data, matching shadow intent and a future-only one-tap rollback drill.

## Verification

Implementation must add and execute named tests, not merely source assertions:

- `ios_process_epoch_is_stable_and_entropy_failure_refuses`: concurrency-safe
  initialization and a failed OS entropy read cannot yield an epoch.
- `same_process_wal_reopen_preserves_epoch_and_elapsed_time`: a real recovered
  anchor retains continuity and does not renew issued deadlines.
- `ios_anchored_wal_refuses_another_process_before_guest_admission`: separate
  processes exercise the actual iOS profile and retained WAL, with an explicit
  executed marker and zero fresh guest/dispatch on refusal.
- `unanchored_start_does_not_release_product_unknown_identity`: first anchoring
  cannot release an existing UNKNOWN mapping or resubmit its original command.
- Retain existing reboot/rollback/overflow, startup fencing, pointer conformance,
  guest/proof and zero-skip native simulator tests. Synthetic clock injection is
  not physical sandbox proof.
- On the signed physical application, require startup past clock anchoring,
  genuine guest intent and matching isolated shadow recording; then lock,
  background/suspend, relaunch/protected-data and retained-identity checks.
  Exercise future-only rollback before selected-default qualification.

## Approval

Independent design review `deleg_1c8b006a` approved the conservative approach,
with the anchored/unanchored distinction and all safety conditions above.
That review is not exact-head document approval, implementation evidence,
release approval or physical qualification. Acceptance requires this ADR's
independent exact-head review and normal required CI/merge queue before code.
