# Generic factory ticket interface — DRAFT 0.1

**Status: DRAFT; revisit with policy at M3, not frozen at v0.**
[ADR-0005](../docs/adr/0005-policy-promotion-tickets.md), [PR #40](https://github.com/softmax-universe/coglet/pull/40).
Imports [common conventions](conventions.md) and shared control definitions from
`messages/promotion.schema.json`. `messages/tickets.schema.json` and
`messages/tickets.cddl` define shapes. Concatenate tickets, promotion and common
CDDL once each, in that order; select `tickets` or `ticket-<definition>` (the
ticket record is `ticket-ticket`). Edit-path uniqueness is checked separately as
a semantic invariant in both representations, including submit/amend payloads.
Wire is deterministic CBOR; JSON is a debugging/fixture view. These are draft
payloads, not a provider API or currently legal frozen hub-wire operations.
Logical names are `ticket.submit`, `ticket.update` (subscription), `ticket.answer`,
`ticket.amend`, `ticket.cancel`, `ticket.result` (read latest durable result).
A later integration ADR must negotiate framing and per-method grants before
network use, including authenticated actor and ticket context on every debug,
status and export read. Transport authentication alone is not ticket authority.
Ticket termination/revocation must close reads as well as writes; there is no
non-ticket fallback for ticket-scoped access. Frozen hub rules remain unchanged.

The fast loop submits; the factory consumes and reports. An adapter can implement
this interface with any scheduler, including a future caos implementation. There
are **no provider fields**, worker profiles, gateway paths, model names or board
identifiers. Provider mapping stays private to the adapter. Unknown fields are
rejected, not tunneled through an unrestricted `metadata` escape hatch.
The factory MUST NOT promote. A done result is input for a separate authorized
[kernel propose](promotion.md), never proof of deployment or user approval.

## Ticket

The submit request has `version:"0.1"`, `correlation_id`, `op:"submit"` and
`ticket` with all these fields (records closed):

| Field | Shape / meaning |
|---|---|
| `target` | `{coglet, repo, base}`: enrolled coglet, registry repo id and full immutable base commit |
| `goal` | `{description, acceptance}`: nonempty plain text and nonempty array of nonempty testable criteria |
| `context` | array of `{kind, ref}`; kind=`world_snapshot`, `transcript`, `trace` or `document`; immutable access-controlled references only |
| `capabilities` | `{edit_paths, debug_read, debug_write}`: unique arrays of repo-relative paths and booleans; read/write independent grants |
| `gates` | `{required, human_approval}`: nonempty unique array of `journal_replay` / `headless_game`, always includes replay; boolean |
| `budget` | positive uint64 `time_ms`, `spend_microunits`, and `currency` (three uppercase ASCII letters) |

Edit paths name allowed file/subtree roots (prefix only on a path-segment boundary),
not globs or shell patterns. Relative ASCII segments use letters/digits/underscore/
hyphen/dot, excluding `.` and `..` segments; no leading slash, empty segments,
backslashes, drive letters or trailing slash. Symlinks resolving outside the repo
or scope are denied. Empty edit_paths is valid for analysis-only work, but cannot
produce changed code. Source/prompts/manifest edits require explicit matching
roots. Debug requests always pass through the hub and its current grants; a
requested ticket capability is a ceiling, never authority by itself. Revocation
closes streams immediately. Writes queue between steps and have hub audit records.
No raw `/data`, transcript, secret or credentials in progress/error fields.

The factory intersects requested edit/debug scope with current operator grants,
checks budget admission, and rejects policy-incompatible gate requests. The
effective checks are the union with target policy (and human approval is OR),
never a way to weaken it. Budget is a hard ceiling including planning, build,
gates and paid in-flight work. Time starts at submit admission and includes needs-
input pauses; spend uses integer millionths of the stated currency, with no
implicit conversion. Reservations must account for in-flight charges; inability
to bound charges means do not start them. Budget exhaustion stops further work
and yields failed, not silent cap renewal.

## Operations, receipts and streaming

Submit returns promptly after durable admission, not build completion. Generic
response fields are `version`, `correlation_id`, `op`, `ok` plus either common
`error` (`ok:false`) or `receipt` (`ok:true`). A successful receipt is
`{ticket_id, revision, state, seq}`: ticket ids use the common opaque id grammar;
revision and seq are positive uint64, initially 1. Other mutating requests add
`ticket_id` and `expected_revision`. The authenticated owner controls who may
answer, amend and cancel; possession of the id alone is not access.

- **Update**: request `{version, correlation_id, op:"update", ticket_id,
  after_seq}` subscribes/reconnects to a durable ordered event stream. `after_seq`
  is uint64 (zero for the start); replay all later events then follow live.
  Events use the subscribed correlation id, `ticket_id`, `revision`, strictly
  increasing `seq`, `state`, nonempty `summary`, and `question` iff needs_input.
  A question is `{id, prompt}` with an opaque id. Initial admission emits queued
  seq 1. States: queued → planning → building → gate_running → done, with forward
  phases skippable, repeated progress in the same phase, needs_input pauses and
  failed/abandoned exits. Needs-input answer resumes the interrupted phase.
  Nonterminal phases cannot move backward except an accepted amendment.
- **Answer**: request adds `question_id`, `answer` (nonempty text). Only the open
  question of that revision in needs_input is answerable. It consumes the question
  once, emits the resumed phase, and does not implicitly approve promotion or
  increase budgets. A user approval must use the kernel attestation flow.
- **Amend**: request adds a complete replacement `ticket`, not a merge patch.
  Target coglet/repo/base cannot change. Admission fences/cancels old work and
  open questions, increments revision, returns queued and emits a new event.
  No old-revision candidate, gate report or answer can finish the new revision.
  Effective authority cannot expand without a separate authorized grant; budget
  increases require separate budget authorization. Caps remain cumulative from
  original admission (no clock/spend reset); already consumed time/spend must
  fit the replacement caps. Stale revisions fail `conflict` with no partial amend.
- **Cancel**: request adds nonempty `reason`. Admission increments revision to
  fence outstanding workers, revokes ticket debug streams, stops new work and
  requests cancellation of in-flight work. Receipt state is `cancelling`; a later
  `abandoned` result certifies cancellation acknowledgment/quiescence. Until then,
  keep accounting/reservations and report cancelling, not safely stopped. A late
  worker cannot publish done after cancel. Already terminal tickets conflict on
  a fresh cancel (identical-id retries recover the receipt).
- **Result**: `{version, correlation_id, op:"result", ticket_id}` reads latest
  result, returning a result record with the request's correlation id. While no
  result exists return common `unavailable` error, not invented done. Result
  records also appear on subscriptions when available. `needs_input` is a
  nonterminal result snapshot; done/failed/abandoned are terminal. Cancelling
  is progress, not a result.

A result has version/correlation/ticket/revision/seq, `state`, `summary`, and:
`candidate` and `gates` iff done; `question` iff needs_input; common `error` iff
failed/abandoned. Done candidate is `{commit, signature_ref}`: signed built commit
containing Wasm, never a branch name. Gates use the shared report shape and must
bind candidate and original base; all effective required checks must pass. The
kernel independently revalidates policy/evidence. Needs-input leaves no candidate;
failed/abandoned results cannot be passed to propose. Progress and result seqs
share one log: the result is the same state transition as its terminal/question
update and uses the same seq, not two distinct transitions.

## Durability, concurrency and retention

Factory admission atomically persists the mutation, receipt and progress outbox
before dispatching. Deduplication key is (authenticated submitter, correlation_id)
for submit, (authenticated actor, ticket_id, correlation_id) thereafter. Identical
requests recover the same receipt; conflicting reuse fails `conflict`. Expected
revision serializes answer/amend/cancel; only amend/cancel increment revision.
Worker output additionally checks current revision/state, so same-revision stale
planning output cannot overwrite done. Serialize result-vs-cancel: whichever
commits first wins and the loser observes terminal/conflict, never both results.

Receipts, events, questions and results survive restart and are retained for the
ticket lifetime. An operator may retire only terminal tickets; then retain a
tombstone rejecting their ids/old requests with `not_found` and never reuse ids.
Retiring a ticket also retires replay (not a silent gap); active tickets MUST
serve every seq after the requested cursor. A cursor beyond the latest seq is
`invalid_argument`. Subscription disconnect never cancels work. Transport retry
is not admission of new work. Authorization is rechecked on every stream/read.
The factory may be restarted/replaced without changing any of these semantics.

## Fixtures and non-claims

`fixtures/tickets/cases.json` includes all six requests, receipts, progress,
conditional result branches, scoped paths and invalid provider-field injections.
`regressions.json` covers final-newline lexical rejection with positive controls
and duplicate edit paths. `spec/tests/test_control.py` validates every shape using
both actual CDDL and JSON Schema, then checks local cross-field invariants and
golden CBOR and selected/public-root shape agreement for handwritten negatives.
The harness codec checks all goldens and wire negatives; a loopback transport
case checks the core golden payloads only, not authorization or lifecycle.
It does not execute a scheduler. The future factory/mock shared
black-box suite must test durable submit retry, progress reconnect, question
revision races, amendment fencing, cancellation quiescence, result-vs-cancel,
budget exhaustion including in-flight spend and revoked debug grants.
