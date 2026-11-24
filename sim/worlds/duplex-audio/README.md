# sim/worlds/duplex-audio

Linux Face mock body. No microphone, speaker or provider session.

The implementation lives in `sim/coglet_sim/duplex.py` (deterministic engine) and
`sim/coglet_sim/host.py` (surface-protocol adapter on the Python SDK). Named
pytest modules under `sim/tests/` drive every `spec/fixtures/speech/timelines.json`
script and every `conformance/speech/` case through this surface.

```sh
cd sim
uv run --locked pytest
uv run --locked python -m coglet_sim.host   # stdio surface; set COGLET_DUPLEX_CASE
```

Playback uses scripted media positions plus a small virtual render latency. Heard
text is the conservative aligned-span prefix (never a duration ratio). Replay of
the same timeline yields an identical event journal.
