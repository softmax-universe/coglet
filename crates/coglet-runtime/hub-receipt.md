# Device `hub.receipt` (lifecycle operations)

`DeviceHandler` serves frozen hub 0.1 `hub.receipt {operation_id}` for operations
it admitted under `hub.git.push`, `hub.install`, `hub.start` and `hub.stop`
(`src/receipt.rs`). `hub.status` is an observation and has no receipt. It does
**not** serve `hub.actions`, inbox, cancel or backpressure receipts.

## Contract a poller can rely on

| Situation | Reply |
| --- | --- |
| Original actor/ticket/target and a current `hub.receipt` plus original-method grant | `reply` with the exact receipt committed at admission |
| Unknown ID, other ticket/no ticket, other actor, caller lacking the original method's grant | `error` `not_found` (identical for all; existence is hidden) |
| Caller with no current `hub.receipt` scope/grant, unpinned target, bad peer | `permission_denied` (independent of whether the ID exists) |
| Missing/invalid `operation_id` or missing context actor | `invalid_argument` |
| Same ID, changed intent (any method/params/actor/ticket) on the *operation's* own method | `conflict` (from the operation, not from `hub.receipt`) |

Receipt body is exactly `{operation_id, state, step, error}`. It never carries a
generation or commit (ADR-0019); `hub.status.live_commit` is not applied-generation
evidence.

## State semantics

* `hub.install`, `hub.start`, `hub.stop`: the receipt is written in the same WAL
  frame as the effect it records, so it is born `applied` (or `failed` for a stop
  whose cancellation is pending). There is no durable "admitted but not yet
  effective" window on this device, so these methods never produce `queued`.
  Receipts are immutable and survive restart; an identical retry returns the
  stored receipt without re-executing.
* `hub.git.push`: the receipt commits with the accepted ref/audit and is
  `queued`. It stays `queued` for its lifetime: an accepted ref is not boundary
  activation, and activation is attested by a qualified adapter (ADR-0019), not by
  this module. Never poll a push receipt expecting `applied` from this device.
* `step` of an applied receipt is the last committed step at admission, so
  `receipt.step <= hub.status.step`; no snapshot atomicity is claimed.

## Persistence and scope

`install_scope` and per-operation `scope` (`actor`, `ticket`, `method`) are stored
in the managed WAL state with the receipt. Push receipts are scoped through the
accepted-ref audit row (`AcceptedRefStore::push_receipt_for_scope`). Receipts
written without scope cannot be read through `hub.receipt` (fail closed).

## Qualification and remaining acceptance gate

| Requirement | Executed test / boundary |
| --- | --- |
| Original scope, immutable terminal record, receipt step <= status | `hub_receipt_original_scope_roundtrip` |
| Wrong actor, ticket, target, original-method grant | `hub_receipt_denies_cross_identity` |
| Same operation ID, differing method/ticket | `hub_receipt_conflicting_intent_typed` |
| Install retry cannot borrow a different ticket's receipt or reuse a stop ID | `hub_receipt_install_retry_cannot_borrow_scope_or_reuse_stop_id` |
| Terminal retry does not append another effect | `hub_receipt_identical_retry_no_reexecute` and `hub_receipt_cogletd_sigkill_recovers_original_scope_and_no_duplicate_install` |
| Actual daemon SIGKILL with queued push; recovered original-scope lookup | `hub_receipt_cogletd_sigkill_recovers_original_scope_and_no_duplicate_install` |
| No activation from admission/restart alone | `hub_receipt_queued_never_applied_without_boundary`, `hub_receipt_git_push_queued_not_applied` |
| Frozen response-schema corpus and non-vacuous malformed request handling | `hub_receipt_request_validation_and_fixtures` |

The response-schema fixture suffix `-unknown` means an extra unknown **field**,
not an unknown operation lookup; the latter is a separate actual-receiver
`not_found` assertion. Malformed requests call the production `DeviceHandler`
directly because `Session::request` refuses to send malformed envelopes. Positive
and original-scope negative receipt requests traverse real iroh sessions.

**Not yet qualified:** a single durable queued operation surviving SIGKILL and
then transitioning to terminal `applied`. The daemon test above recovers a queued
push and then installs via a *different* operation; it does not claim that install
completes that push. Frozen hub 0.1 requires a router ref to become applied only at
a verified runtime boundary (`spec/hub-protocol.md`, lifecycle ordering and Git
control plane). PR125's device slice has no queued router-ref application API.
An accepted-ref record or matching live commit cannot substitute for that boundary
or attest an applied generation. Do not advance an ADR-0018 deploy saga past a
queued child using this implementation, and do not treat this test matrix as the
full card acceptance gate.
