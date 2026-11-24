# sim/worlds/duplex-audio

Linux Face mock body. No microphone, speaker or provider session.

The implementation lives in `sim/coglet_sim/duplex.py` (deterministic engine) and
`sim/coglet_sim/host.py` (surface-protocol adapter on the Python SDK).

```sh
cd sim
uv run --locked pytest
uv run --locked python -m coglet_sim.host   # stdio surface; set COGLET_DUPLEX_CASE
# simulator -> runtime heard history + offline replay (needs uv):
cargo test --locked -p coglet-runtime --test sim_duplex_heard -- --ignored
```

Named tests:

- `tests/test_duplex_timelines.py`: every `spec/fixtures/speech/timelines.json`
  script (events, played/heard/truncation, replay from every intermediate
  snapshot), per-item heard history, stop semantics.
- `tests/test_duplex_conformance.py`: every `conformance/speech/` case through the
  AF_UNIX surface with the shared conformance runner.
- `tests/test_duplex_wire.py`: real `Surface.receive` regressions for every
  advertised operation (accept, reject, terminal result, and cancellation or
  fencing where the spec defines them). See the contract table below.
- `crates/coglet-runtime/tests/sim_duplex_heard.rs`: the simulator drives a
  durable RuntimeService; the runtime journals `runtime.heard` from its own owner
  reads (finished aligned item, unaligned item -> empty text, interrupted item
  -> "Hello" at 1200 of 3000 ms, queued item never heard), offline replay
  reproduces every record hash, and two independent runs give identical
  `heard.cbor` bytes.

Timing: render starts 20 ms (wall) after admission; virtual media then advances
10x faster than wall time. Wire positions are virtual media milliseconds.
Heard text is the conservative aligned-span prefix (never a duration ratio);
text without voice alignment has unknown alignment and empty heard text.
Exclusive resources are advertised only on their owning entry
(`speech.control`, `expression.control`); `speech.history`/`speech.work` still
need the leases in grants. Scenario `heard-history` exists for the runtime test.

Not covered: acoustic timing, real echo cancellation, and provider sessions
(`speech.session_*`, `speech.tool_call`, `speech.tool_result`). Those session
operations are optional and are not advertised. Capture-mode cases advertise
only the empty marker entry, not speech operations.

## Advertised operations

Every operation this surface advertises is implemented. Optional provider
session operations are not advertised. Wire tests use `tests/test_duplex_wire.py`.

| operation | spec | wire test |
|---|---|---|
| speech.start | speech.md:49,56-62 | test_capture_events_accept_valid_and_discard_stale; test_advertised_events_reject_action_admission |
| speech.partial | speech.md:50,56-65 | test_capture_events_accept_valid_and_discard_stale; test_advertised_events_reject_action_admission |
| speech.end | speech.md:51,56-62 | test_capture_events_accept_valid_and_discard_stale; test_advertised_events_reject_action_admission |
| speech.position | speech.md:52,65-71 | test_chunks_play_in_order_with_cumulative_position; test_stop_before_playback_cancels_the_pending_speak |
| speech.barge_in | speech.md:53,87,109-122 | test_heard_truncation_items_are_the_admitted_action_id; test_idle_capture_does_not_invent_barge_in; test_advertised_events_reject_action_admission |
| speech.config | speech.md:25-36 | test_speech_config_read_and_rejection |
| speech.speak_chunk | speech.md:80,84-107,109-117 | test_chunks_play_in_order_with_cumulative_position; test_speak_chunk_bounds_idle_close_and_limit; test_stop_before_playback_cancels_the_pending_speak; test_deadline_expiry_stops_audio_and_fences_playback; test_chunk_expiry_cancels_later_chunks_and_closes_the_stream |
| speech.stop | speech.md:81,109-111 | test_stop_before_playback_cancels_the_pending_speak; test_unknown_stream_stop_is_not_found_and_other_stream_conflicts |
| speech.backchannel | speech.md:15-20,82,100-107 | test_backchannel_plays_each_cue_as_its_own_stream; test_backchannel_conflicts_and_stop_fences; test_deadline_expiry_stops_audio_and_fences_playback |
| expression.set_expression | expression.md:10-22,33 | test_expression_apply_expiry_fence_and_revocation |
| expression.state | expression.md:21-22 | test_expression_apply_expiry_fence_and_revocation |
| speech.heard | speech.md:166-169 | test_heard_truncation_items_are_the_admitted_action_id; test_heard_records_are_removed_on_owner_revocation |
| speech.work_state | speech.md:208,221-223 | test_work_state_read_and_rejection |
| speech.work_intent | speech.md:207,227-244,246-251 | test_work_intent_dedup_and_start_cancels_without_barge; test_work_intent_refuses_a_fifth_live_call |
| speech.work_done | speech.md:210,246-264 | test_work_result_preserves_status_and_rejects_duplicates; test_work_cancelled_emits_one_done_and_rejects_a_result |
| speech.work_result | speech.md:209,257-274 | test_work_result_preserves_status_and_rejects_duplicates; test_stale_mismatched_and_oversized_work_result |

`speech.session_open`, `speech.session_close`, `speech.tool_call` and
`speech.tool_result` are optional (speech.md:124-127) and are not advertised
(`test_capabilities_omit_unimplemented_session_ops`).
