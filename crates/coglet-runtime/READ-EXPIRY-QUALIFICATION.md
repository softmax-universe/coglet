# ADR-0029 runtime expiry qualification

The docs-only decision landed in PR #153 before this implementation. Issued
read deadlines, wire schemas, guest ABI and heard-only reasoning are unchanged.

| Requirement | Named tests (required Rust workspace test job) |
|---|---|
| Deterministic control | `queued_read_reply_control_without_delay` |
| Two test-only 600,000us commits hold an adapter-queued reply past its fixed deadline; one native negative completion | `expired_read_completes_negatively_to_supervisor` |
| Exact exclusive deadline, no early disconnect, caller isolation | `expire_reads_does_not_disconnect_before_native_completion` |
| Late reply tombstoned; no fabricated archive or reasoning release | `reply_at_or_after_published_deadline_completes_negatively_and_keeps_reasoning_fenced` |
| Unconsumed negative cannot cross revoke/kill/disconnect | `expired_heard_completion_is_fenced_by_revoke_kill_and_disconnect` |
| Queue pressure refuses admission, never silently drops negatives | `unconsumed_heard_expiries_backpressure_without_silent_drop` |
| Bounded metadata-only diagnostics, unknown not zero queue depth | `read_expiry_diagnostics_are_bounded_and_metadata_only` |
| Existing aggregate read bound and correlation reuse | `native_read_pending_and_completed_share_one_bounded_queue`, `native_read_is_caller_scoped_and_timeout_discards_late_reply` |

The first six rows live in `src/heard_deadline_tests.rs`, using the existing
per-instance kernel clock and WAL before-commit seam (both cfg(test)). The final
two rows live in `tests/operational_surface.rs`. No workflow edits are needed:
the existing required `rust` job executes workspace library and integration tests.
Stress loops are separate local stability evidence, never required-job loops.

## Observation boundary and bounded failure

`SurfaceSession::take_read_result` is the existing trusted Rust native completion
API; repository search finds no production C-ABI read-result getter or guest read
import. This fix preserves that boundary, not a new wire/native ABI export.
Expiry no longer clears its result before that API can return it. The durable
heard_pending fence remains, so a supervisor must handle failed observation and
may disconnect/re-negotiate; a late successful reply cannot release that fence.
Unrelated health/inactivity/authority/disconnect policy still applies. Negative
completion retention is bounded and session-local, not crash-durable. The existing
configured `supervisor_reads` durable ledger records actual received responses,
not local timeout results; its attempted/no-response recovery is not changed or
claimed fixed here. ADR-0029 explicitly permits the live `take_read_result` path.

Runtime-owned heard expiries now share the existing 128-slot pending/completed
budget with caller-issued reads. A supervisor drains all its owner-scoped results,
including runtime-issued correlation IDs. A caller that does not consume results
gets bounded backpressure, not automatic eviction, fabricated success, or a new
read allowance. Draining a negative frees completion capacity but does NOT clear
the durable heard reasoning barrier. At 4096 lifetime IDs, the existing tombstone
limit requires a new session; no tombstones are evicted to hide pressure.

Each deadline expiry emits one JSON diagnostic to native stderr (write failures
are ignored without unwinding C callers) and retains the latest 128 records via
`read_expiry_diagnostics()` while that session object lives, including after close.
The validated correlation ID and u64 clocks bound each record. The five fields
are event name, ID, deadline, expiry tick and `queued_inbound_frames_ahead: null`.
The adapter-owned outgoing FIFO is not visible at this synchronous boundary:
null means unknown, not measured zero. No text, args, frame bytes or private result
is logged. Emissions are once per expired ID (at most 4096 per session); stderr
retention belongs to the native host's log sink, not this in-memory ring.

## Baseline and scope

Before the production edit, the control passed and both newly named negative
completion tests failed on main (missing result and early disconnect). Raw logs
are retained on the owner card. No sleep, production delay switch or timeout
increase is used.

These are runtime-boundary tests with real WAL commits, not physical Face audio,
Swift FIFO or real-engine/C-pointer proof. Face owner t_01a2084da345e698 consumes
the merged runtime revision and requalifies its own original C-ABI proof, delay
experiment and 20 plain runs. Runtime acceptance does not change Face acceptance.
