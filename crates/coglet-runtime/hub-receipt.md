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
