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
* `hub.git.push`: admission commits a `queued` receipt, retained original request
  and verified bundle with the accepted ref/audit. For an uninstalled target,
  the runtime driver performs a fresh validator pass, rechecks current authority
  and delegation, then atomically commits explicit pre-install quarantine boundary
  evidence and the ORIGINAL operation's `applied` receipt (ADR-0026). This never
  executes code or sets `live_commit`. Install requires that exact applied stage.
  Pushes to installed targets remain queued: installed code activation is owned
  by the separate step-boundary producer, not this module.
* `step` of an applied receipt is the last committed step at admission, so
  `receipt.step <= hub.status.step`; no snapshot atomicity is claimed.

## Persistence and scope

`install_scope` and per-operation `scope` (`actor`, `ticket`, `method`) are stored
in the managed WAL state with the receipt. Push receipts are scoped through the
accepted-ref audit row (`AcceptedRefStore::push_receipt_for_scope`). Receipts
written without scope cannot be read through `hub.receipt` (fail closed).

## Qualification boundaries

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
| Initial original push completes before install, without live code | `hub_receipt_preinstall_original_push_completes_before_install` |
| Original push retry/restart retains exactly one audit, intent and boundary | `hub_receipt_original_push_retry_retains_single_boundary` |
| Missing async recovery executor fails startup closed | `hub_receipt_constructor_refuses_missing_recovery_runtime` |
| Terminal SQL UPDATE/DELETE/REPLACE refusal and missing-state control | `operation_receipt_storage_is_completion_only_and_terminal_immutable` |

The response-schema fixture suffix `-unknown` means an extra unknown **field**,
not an unknown operation lookup; the latter is a separate actual-receiver
`not_found` assertion. Malformed requests call the production `DeviceHandler`
directly because `Session::request` refuses to send malformed envelopes. Positive
and original-scope negative receipt requests traverse real iroh sessions.

The daemon SIGKILL test uses a test-local second-validator barrier: it kills after
durable queued admission, reopens still queued, proves install refusal, releases
verification and requires the SAME push ID applied with exactly one durable
boundary. A later install is a distinct operation, not evidence completing push.

**Not qualified by this module:** installed-target code activation, generation
attestation, inbox/actions/cancel receipts or the complete hub conformance suite.
Never advance an ADR-0018 saga on a queued child. An accepted-ref record or matching
live commit alone cannot substitute for explicit boundary evidence. Device startup
requires an active Tokio runtime to retain recovery executor custody.
