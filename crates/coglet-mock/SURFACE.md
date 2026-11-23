# Mock surface fixture adapter

`surface::MockSurface` is a `coglet_surface_rust::transport::ControlPeer` wrapping
an actual SDK `Surface`. It never constructs expected protocol replies.

- `from_fixture(json, case, state_dir)` uses `AdapterFixture`; `case` selects only
  `startup_epoch_by_case`. Without `mock_timeline`, it uses `SemanticBackend`.
- `scripted(config, state_dir, Arc<ScriptedBackend>, timeline)` records admitted
  requests. `tick_at(now_us)` replays due commands in order; `install_timeline`
  atomically replaces remaining commands (absolute times, no past events).
- A `complete` command applies the recorded request to `SemanticBackend` at that
  instant, which emits actual semantic completion via the SDK sink. Unknown,
  cancelled, expired or previously completed IDs cannot apply an effect.
- Scripted text capture events pass through SDK normalization. Captured epoch is
  explicit, never assigned from the epoch at delivery. Provider callbacks retain
  the captured session ID. No audio playback/provider session support is claimed.

Optional `mock_timeline` in the normal public fixture config:

```json
{
  "events": [
    {"at_us": 20000, "command": "complete", "id": "22222222222222222222222222222222"},
    {"at_us": 30000, "command": "native", "captured_epoch": 1,
     "event": {"kind": "speech_start", "utterance": "44444444444444444444444444444444", "position_ms": 0}},
    {"at_us": 100000, "command": "kill", "reason": "local operator"}
  ]
}
```

Other commands: `revoke` with `coglet`. Native kinds: `speech_start`,
`speech_partial`, `speech_end`, `session_ended`, `tool_call`. Session kinds are
subject to SDK session normalization; this semantic backend does not advertise
session-open or implement provider sessions. Timelines reject unknown fields,
invalid IDs, descending timestamps, events beyond 60 seconds, more than 4096
events, per-event JSON beyond 64 KiB, or total JSON beyond 1 MiB. Equal-time
commands retain input order. Core expiry runs before effects at an exclusive
expiry/deadline bound.

```sh
COGLET_FIXTURE_CONFIG="$PWD/spec/fixtures/surface/adapter-config.json" \
COGLET_FIXTURE_KILL_AT_MS=100 \
cargo run -p coglet-mock --bin coglet-mock-surface -- --stdio --transport in-process
```

`--transport stdio` directly bridges framed stdin/stdout to the
core. `--transport in-process` actually uses the SDK's `in_process_pair`. Both
use SDK `read_frame`/`write_frame`, a bounded reader channel, and a real elapsed
microsecond clock. `COGLET_FIXTURE_CONFIG` is required; optional
`COGLET_FIXTURE_CASE` only chooses startup epoch. The public fixture sets hello
clock explicitly to zero. The binary creates a private unique state directory
under `TMPDIR` and removes it on ordinary EOF/error. Diagnostics use stderr only.
Abrupt process termination cannot guarantee directory cleanup.

## Scope and callback safety

The SDK gives each native sender an immutable acquisition generation and owner.
Grant replacement invalidates old senders and purges queued old observations;
provider events additionally need their captured active session ID. The mock also
checks its scripted `captured_epoch` before enqueueing. Native dispatch tokens
are distinct from owner-scoped wire IDs; a scripted completion with a wire ID
shared by multiple live owners is rejected as ambiguous rather than completing
someone else's action. Hardware adapters must capture the sender when starting
work, not fetch a current sender inside an old callback.
No physical audio, provider-session or native C ABI conformance is claimed.
The stdio fixture creates fresh durable state per case; crash/restart proof is
in the SDK integration tests, not implied by the wire case passes.

Validation: `cargo test -p coglet-mock`; `cargo clippy -p coglet-mock --all-targets
-- -D warnings`. Integration tests exercise binary framing, both transport paths,
clock-zero hello, the 100ms local kill, post-kill rejection and state cleanup.
