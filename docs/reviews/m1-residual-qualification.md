# M1 residual B-gate qualification evidence map

Owner: `t_01a20c2f7b9bb95a`. This record supplies bounded evidence and explicit ADR dispositions for
[`spec-v0-m1-gates.md`](spec-v0-m1-gates.md). ADR-0014's isolated echo proof
remains unchanged. None of the component results below authorizes an otherwise
withheld production route.

## New component tests

The ordinary workspace test discovery includes these integration targets in the
required `rust` job (`scripts/ci/cargo-test --workspace --locked`). Required CI
execution on test commit `c569486519a9c0c8ef82b5b3c083eaee527a6a66` passed
([rust run 45396006313](https://github.com/softmax-universe/coglet/actions/runs/45396006313)).
The final PR head still requires its own green CI and independent exact-head
review before landing; local execution is not CI qualification.

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

- `unknown_admission_no_new_id`: after reopening an unresolved prepared action,
  a substituted ID with no durably pending intent is refused; the original ID
  still retrieves identical bytes and cannot synthesize an unadmitted success.
  This does not prohibit a guest from issuing a distinct legitimate action.
- `terminal_rejection_allows_new_id`: a retained authoritative rejection survives
  reopening; consuming its wake permits a guest-emitted new action ID to be
  prepared, while retrying the original ID retrieves its historical bytes and
  repeating its rejection creates no new wake.

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

### Router `m1_synthesis`

- `synthesized_success_without_authority_denied_with_authenticated_positive_and_audit`
- `authorized_terminal_failure_preserves_identity_dedup_and_audit`

Both execute enrollment and outcome submission through real authenticated iroh
sessions, signed registration/delegation, and actual router dispatch to a fixture
responder. Unknown and retained-but-unattempted action IDs are refused. After
positive dispatch admission, wrong source, foreign target, both correlation
mismatches and unknown ID are refused without queue/audit mutation. Authorized
success and failure each retain one exact outcome and an authenticated
peer/target/offset audit. Exact batch retries are idempotent; a second batch with
the same terminal or a schema-valid conflicting cancellation is rejected with
`conflict`, without an additional delivery or audit. Reopening preserves the
exact delivery token, terminal and audit. This is router relay authority, not
physical-effect proof; the fixture responder only returns action admission.
Initial cancellation fixture omitted its required error and reached schema
refusal instead of the conflict branch; corrected before the passing run. No
historical production RED or mutation sensitivity is claimed.

## Residual disposition and live ownership

[ADR-0023](https://github.com/softmax-universe/coglet/blob/main/docs/adr/0023-m1-residual-withheld-routes.md)
was accepted through independently reviewed, merge-queued
[PR #123](https://github.com/softmax-universe/coglet/pull/123), merged as
`8e1942b958fbd7f923b221a0e8000f98e1772110`. These moves do not
claim that the missing positive tests ran. The inventory's primary-owner
column still names the original Phase-2 cards, because
`test_m1_inventory_has_one_disposition_and_owner_per_row` requires exactly
one of those frozen IDs there; live successors are named in the proof
column, this map, and ADR-0023.

| Ledger row | Disposition / owner |
|---|---|
| H-transform | **QUALIFIED bounded native Linux initial publication** by the ADR-0023 successor `t_01a20d98a7fb7229` under ADR-0025. The [live ledger](spec-v0-m1-gates.md#m1-linux-route-evidence-adr-0014) names real rewrite/sign/Store tests, protected configuration/live authenticated CI provisioning, the full native x86_64/aarch64 960-case corpus, identical signed-archive runtime filesystem/journal/trap/payload-chain replay and `unqualified_transform_publication_refused`. Required Rust aggregation rejects missing/skipped/filtered or mismatched evidence; source proof and limits are in [BUILD-R3](../../crates/coglet-hub/BUILD-R3.md#required-production-pathnative-qualification). Identity fixtures remain provenance-binding only; absent/expired provisioning, installed hot-update, arbitrary sources, AOT and Apple/device qualification remain refused/unclaimed. |
| H-synthesis | **Bounded component proof**: the real-iroh success/failure matrix above covers callable event/action retained authority and audit. Runtime matrix complements it. Final CI/review/landing remains this card's gate, not an underlying physical-adapter qualification. |
| H-private | Local private event/ack control above supplements existing immutable-owner registration proof. **MOVED, WITHHELD** transfer/read/back-channel positives to `t_01a20d98a8bca347` under ADR-0023. Existing frozen debug/owner-only relay implementation needs no new ADR; new transfer or changed semantics does. |
| H-relay-grants | Local target/actor/ticket/expiry/revocation controls above are direct trusted-context component tests. **MOVED** broader authenticated all-method matrix to `t_01a2084063ee0c84` under ADR-0023, against reference and mock. Joined deployment also needs `t_01a20bcfe6db20c3` / `t_01a20bd00f035b09`. |
| R-retry | Five original ledger obligations map below; three process-exit tests add exact named WAL boundaries. No arbitrary fsync/power-loss or physical exactly-once claim. |

### R-retry original ledger obligation mapping

These are semantic mappings to actual tests, not claims all proposed names
exist. The source ledger enumerates five obligations; it does not enumerate
an additional arbitrary pre/post-fsync crash matrix.

| Original proof name | Executed test(s), target and limit |
|---|---|
| `clock_resample_retry_same_bytes` | `operational_surface::byte_identical_retry_after_clock_advances`; `native_operational::ffi_byte_identical_retry_after_clock_advances_and_reconnect_no_duplicate_act` and socket counterpart. Once-translated immutable bytes, not physical exactly-once. |
| `crash_after_persist_before_send` | Same-name `m1_retry_crash` parent executes `retry_crash_child`, requires exit 73 after retained dispatch, then reopens and compares exact bytes with no replacement grant. |
| `reconnect_reconciles_old_epoch` | `operational_surface::durable_grant_epoch_survives_reconnect_and_restart` refuses stale grant epoch after reopen; the two native retry tests above preserve saved requests across clock/reconnect; `m1_retry_crash::crash_after_accept_before_outcome_reconciles_original_admission` preserves admitted identity after process exit. |
| `unknown_admission_no_new_id` | Same-name `m1_retry_crash`: substituted ID lacking durably pending intent refused, original bytes retained, unadmitted success refused. A distinct legitimate guest intent is not prohibited. |
| `terminal_rejection_allows_new_id` | Same-name `m1_retry_crash`: durable rejection, guest consumption and fresh guest-emitted ID succeeds, old bytes retained, duplicate rejection creates no wake. |

`crash_after_outcome_before_consumption_keeps_one_terminal` adds the third
process-exit boundary. `native_operational::ambiguous_wal_failure_fences_native_clock_and_retained_dispatch`
remains the existing ambiguous-WAL fence control. All targets above are ordinary
required-workspace test discovery; the ignored crash helper is explicitly
invoked by its parents, not silently skipped qualification.

Production `cogletd` cannot enable `test.echo`. Unrelated engine, provider,
native speech and device gates retain their original owners and limitations.

## Local verification commands

```sh
cargo test --locked -j 2 -p coglet-runtime --test m1_retry_crash
cargo test --locked -j 2 -p coglet-hub --test m1_authority_matrix --test m1_synthesis
cargo clippy --locked -j 2 -p coglet-runtime -p coglet-hub \
  --test m1_retry_crash --test m1_authority_matrix --test m1_synthesis -- -D warnings
cargo fmt -p coglet-runtime -p coglet-hub -- --check
git diff --check
```

The component commands above passed on the test additions. The cited CI run
also passed on the test commit; final-head review and landing are recorded on
PR #120 rather than inferred from this document. No historical RED, mutation
sensitivity, physical effect or all-platform qualification is claimed.
