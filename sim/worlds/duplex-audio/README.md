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
- `tests/test_duplex_wire.py`: real `Surface.receive` regressions: playback items
  are the admitted action IDs (varied IDs and correlations), `speech.stop` cancels
  queued/playing speak actions before succeeding, finished actions stay
  unchanged, and stopped/revoked playback callbacks are fenced.
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

Not covered: acoustic timing, real echo cancellation, provider sessions and
backchannel (rejected), which need native hardware gates.
