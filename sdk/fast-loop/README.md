# coglet-fast-loop

Reusable guest-SDK pieces for a low-latency control loop with asynchronous
planning (docs/plan.md, Phase 4 "fast loop"). The Mindister avatar and the Baby
Bear conversation route plug a `Controller` into `FastLoop`; the loop owns the
bounded-compute and async-plan mechanics.

- **Fast path, every observation.** `Controller::decide` runs locally against a
  deterministic work `Budget` (units the controller charges itself; the loop cannot
  verify the controller charges honestly). If a charge
  is refused the loop emits the controller's O(1) `fallback` instead and counts
  the tick as degraded. The tick itself makes no LLM call and never waits. A step
  that carries a new `runtime.inbox` wake first reads and validates the inbox (capped
  at 1 MiB, not charged to `Budget`), then ticks; steps without a new wake never open it.
- **Slow path, asynchronous.** With nothing pending, `wants_plan` stages one
  `llm.complete` request. The reply arrives in a later step as a `runtime.inbox`
  wake over `/data/inbox.jsonl`; only the awaited request id from the configured
  sender is accepted, then `parse_plan` validates it. Failure, rejection, timeout
  and a host-refused request back off and never cost a tick its action. Late,
  duplicate, foreign and unrelated inbox messages are counted and ignored.
- **State.** One JSON file under `/data` (tick, plan, pending request, inbox
  cursor, stats; serialized in field declaration order). Replay of the recorded batches and inbox bytes reproduces every
  effect and every state byte. Inbox lines are validated against the ABI schemas
  (message, event, `llm-reply`; canonical LF lines; contiguous index; sender equals
  event source) before any is applied. A damaged, oversized or unreadable inbox is
  counted (`inbox_faults`), its lines up to the wake are skipped and the fast path
  carries on; the pending request times out. The ABI has no inbox rotation, so a
  coglet whose inbox passes 1 MiB loses its slow path (and runs on reflexes) until
  the runtime offers a way to trim or seek; that is a known limit.

The budget is a work count, not wall time. It sits inside the runtime's manifest
`compute` / `wall_time_us` limits, which stay the hard bound on any step.

## Named tests

`cargo test --locked -p coglet-fast-loop --test fast_loop`

- `avatar_meets_tick_budget_while_planning_continues_asynchronously`: on the
  fixture map (`tests/fast_loop.rs`, a wall between start and goal) with a mock
  LLM that answers 12 ticks late: the goal is reached, no tick charged more than the
  stated 64 work units, no step exceeded the stated 50 ms host-measured wall budget
  (debug build; typical steps are far below), the plan landed exactly 12 steps
  after the request, and the avatar acted on ticks while the request was in flight.
- `reflex_alone_stalls_at_the_wall_and_the_async_plan_is_what_gets_around_it`:
  negative control. With the host refusing LLM requests the avatar stalls at the
  wall; with a 40-tick planner it arrives.
- `replay_reexecutes_each_step_without_llm_or_world_and_matches_every_effect_and_state`:
  fresh instance, recorded inputs only (no world, no LLM): effects and file hashes
  match at every step; a tampered reply and a changed seed are detected.
- `exhausted_budget_degrades_to_the_fallback_and_never_overruns`
- `failed_foreign_duplicate_and_malformed_replies_never_stall_ticks`
- `unanswered_request_times_out_and_its_late_reply_is_ignored`
- `refused_request_costs_neither_the_action_nor_the_tick`
- `plan_expires_after_ttl_and_a_finished_plan_is_dropped`
- `damaged_inbox_is_counted_and_never_stops_the_fast_path` (missing line, CRLF, no
  final LF, index gap, sender/source mismatch, schema-invalid reply, oversize)
- `inbox_is_not_read_unless_a_wake_names_a_new_line`
- `action_deadline_overflow_is_refused_not_wrapped`
- `zero_backoff_retries_a_refused_request_on_the_next_tick_and_nonzero_waits`
- `bad_observations_are_counted_and_skipped_without_effects`

## Limits

The harness is a transactional test host and a mock LLM, not the native runtime or
a WAMR engine, and replay means re-executing the guest against recorded inputs. The
fixture controller and map are test code; this crate does not ship a game policy.
Builds for `wasm32-wasip1` (checked locally, not yet a CI step; a product guest crate
should build its own guest). The 50 ms wall
bound is a coarse regression guard on shared CI hardware, not a latency claim for a
device. Mindister's avatar adoption and the Baby Bear conversation route are
separate cards.
