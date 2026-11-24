# Heard-read publication deadline qualification

The runtime-owned 1,000,000 µs heard-read allowance begins after the event/barrier
WAL commit, immediately before local read publication. Native transports already
establish a durable runtime clock anchor; publication resamples that same kernel
boot/continuous clock without a second WAL write. Explicit logical-clock callers
without a native anchor retain their supplied time. No guest budget, immutable
action deadline, issued read deadline, grant expiry, queue bound, health/inactivity
rule or adapter exclusive-deadline admission check is relaxed.

## Specification preflight

`spec/surface-protocol.md` requires exclusive issued deadlines and late-reply
refusal; it specifies neither an event-entry origin nor this internal one-second
allowance. ADR-0003 R4 governs guest recorded inputs, not deterministic host wall
scheduling (see its Consequences). ADR-0009 M-C/M-D keeps heard reads private and
journalled and forbids synthetic guest events. No wire/schema/frozen conformance
fixture changes are needed.

## Deterministic requirement-to-test matrix

Run `cargo test --locked -p coglet-runtime --lib heard_deadline_tests` (also runs
in the full default/native suites). The kernel sampler and WAL commit hook are
per-instance, `cfg(test)` only; production has no injected-clock/fault switch.
Tests perform real WAL commits. The hook advances the injected clock *inside*
the delayed commit, not at the deadline assertion; no wall sleep is used.

| Requirement | Named test |
|---|---|
| 600 ms durable work before publication leaves a full issued second; a reply beyond the old deadline commits heard state | `slow_durable_store_gives_heard_read_one_second_from_publication` |
| A reply and later event queued during a 500 ms commit remain FIFO; old issued deadlines are never extended by another slow commit | `queued_replies_behind_slow_store_remain_fifo_and_do_not_extend_issued_deadlines` |
| Reply at/after the published exclusive deadline disconnects and purges private queues/barriers without archive success | `reply_at_or_after_published_deadline_disconnects_and_purges_private_state` |
| Negative owner reads fail closed, never become guessed/empty successful heard history | `negative_heard_reply_is_fail_closed_not_a_successful_empty_observation` |
| Fresh publication rechecks grant expiry and the unchanged three-second inactivity fence | `slow_store_does_not_renew_expired_grants_or_inactivity` |

A local mutation restoring the entry-time deadline made the slow-store, FIFO and
fresh-fence tests fail. The mutation was removed before qualification.

## Run307 classification and evidence limit

The original `cabi-diag14-307/proof.log` records `receive kind=read_reply
now=3176306 pending_reads=3`, followed by `owner heard observation unavailable` and
`send -2`. Unlike runs 201/323/399, this reaches `receive_read_reply`'s negative
owner-observation branch, not `expire_reads`' deadline-disconnect branch. The raw
negative error code/body was **not captured**, so it cannot be classified as
`deadline_exceeded`, nor can its underlying adapter cause be recovered from that
log. The runtime correctly fails closed when an authoritative owner observation
is unavailable. Tests cover `deadline_exceeded`, `not_found`, `permission_denied`
and `unavailable` negatives separately; they do not identify run307's missing
code. No negative-reply fallback or retry was introduced.

Evidence: Face card `t_01a2084da345e698`, hub evidence
`run14-extra-logs.tar.gz` / `cabi-diag14-307/proof.log` and
`run14-operational-findings.md`. The separate 433 trace proves prepublication
persistence latency, not the entire cause of every historical failure. These
regressions qualify shared-runtime control behaviour, not real Face media,
MainActor scheduling or acoustic acceptance.
