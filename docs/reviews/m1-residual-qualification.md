# M1 residual B-gate qualification work in progress

Owner: `t_01a20c2f7b9bb95a`. This record supplements, and does not close,
[`spec-v0-m1-gates.md`](spec-v0-m1-gates.md). ADR-0014's isolated echo proof
remains unchanged. None of the component results below authorizes an otherwise
withheld production route.

## New component tests

The ordinary workspace test discovery includes these integration targets in the
required `rust` job (`scripts/ci/cargo-test --workspace --locked`). Required CI
execution and independent exact-head review are still pending; local execution
is not CI qualification.

### Runtime `m1_retry_crash`

- `crash_after_persist_before_send`: a child prepares and durably retains a
  complete translated request, then exits without destructors before handing it
  to any transport. Reopening retrieves identical bytes without replacement
  grants; an unadmitted outcome is refused.
- `crash_after_accept_before_outcome_reconciles_original_admission`: after a
  child retains admission and exits, a replacement session reconciles the
  original request and records one terminal; another event carrying the same
  outcome payload does not append another inbox wake.
- `crash_after_outcome_before_consumption_keeps_one_terminal`: exit after the
  terminal is retained but before guest consumption; reopen preserves one wake,
  deduplicates its terminal payload and refuses a conflicting terminal.
- `synthesized_success_requires_retained_admission_source_and_correlation`:
  unknown/unadmitted actions, wrong source, outer/inner correlation, unknown ID
  and malformed selected success result are refused. The unchanged authorized
  outcome succeeds, and its duplicate remains deduplicated across reopen.

`retry_crash_child` is an ignored subprocess helper explicitly executed by each
ordinary crash test. Each parent requires its boundary-specific exit code. These
are process-exit/WAL component proofs, not power-loss durability, transport
resend, physical-effect exactly-once, or all crash-point qualification. Event
sequence advancement is preserved when checking duplicate/conflicting terminals.

### Router `m1_authority_matrix`

- `relay_target_without_grant_denied_with_authorized_positive_control`: foreign
  target and unenrolled test peer fail before admission; the exact
  configured peer/target succeeds and owns one queued event.
- `relay_wrong_actor_and_ticket_denied_before_retention`: wrong actor and
  ungranted ticket leave no retained action audit; the authorized local actor
  succeeds, and a retry cannot remap its retained deadline.
- `relay_expiry_and_revocation_fence_identical_retries`: exact event retry is
  refused at expiry and after revocation, with live authorized controls and no
  duplicate event queue entry.
- `relay_private_owner_only_local_delivery_and_ack`: a configured private event
  is delivered only to its owner; another target cannot observe or acknowledge
  it. Owner acknowledgement succeeds.

These tests supply trusted `RequestContext` values directly. Real signature,
Authority and registry checks execute, but iroh peer authentication is not newly
qualified. The private event payload is a generic event fixture, not native
speech/provider qualification. Local delivery/ack is not remote read/debug.

## Remaining obligations (all still owned by this card)

| Ledger row | Still unqualified / next proof or decision |
|---|---|
| H-transform | Real rewrite before build/commit; pinned transform fuzz and cross-architecture qualification; authenticated publication of the transformed artifact. Existing output-bound provenance/validator tests alone do not prove these. No publication route is enabled here. |
| H-synthesis | Router-level complete admitted/unadmitted, source/owner/correlation, terminal-failure, duplicate terminal and retained audit matrix. Runtime outcomes above are supporting component evidence, not a substitute. |
| H-private | Subscription-not-authority, ownership transfer purge/fence, private read/event leakage and remote-debug exclusions with positive controls. The current immutable registry has no ownership-transfer API; adding a protocol/API contract is not authorized by a test. |
| H-relay-grants | Broader method/target matrix over authenticated sessions, including cross-target replay, wrong actor, expiry/revocation and authorized controls. The local component matrix is not all-method wire conformance. |
| R-retry | Explicit no-new-ID-on-unknown-admission and new-ID-after-terminal controls; crash gaps beyond the three named process boundaries above. Keep existing clock-advance, old-epoch reconnect and ambiguous-WAL tests; no claim that proposed acceptance names already exist. |

Any migration of an obligation requires an independently reviewed ADR with its
reason and live owner. The orchestrator has been asked to reserve the next free
ADR number; this document neither reserves one nor changes the frozen spec.
Joined production deployment/relay remains outside this card under the three
owners named in its body. Production `cogletd` cannot enable `test.echo`.

## Local verification commands

```sh
cargo test --locked -j 2 -p coglet-runtime --test m1_retry_crash
cargo test --locked -j 2 -p coglet-hub --test m1_authority_matrix
cargo clippy --locked -j 2 -p coglet-runtime -p coglet-hub \
  --test m1_retry_crash --test m1_authority_matrix -- -D warnings
cargo fmt -p coglet-runtime -p coglet-hub -- --check
git diff --check
```

All commands above passed on the work-in-progress test additions. No historical
RED, mutation sensitivity, full workspace pass, required-CI pass, independent
review or merge is claimed by this record.
