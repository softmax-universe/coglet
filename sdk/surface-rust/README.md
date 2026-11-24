# coglet-surface-rust

Reference Rust SDK core for the Coglet surface protocol (spec-v0, protocol 0.1).
Not published. Transport adapters live in `src/transport.rs`; this crate is the core.

## Use

```rust
let fx = AdapterFixture::parse(&json, None)?;               // trusted supervisor config
let backend = SemanticBackend::new(fx.speech, fx.expression); // or your qualified Backend
let mut surface = Surface::open(fx.config, state_dir, backend)?;
// `Surface` implements `transport::ControlPeer`; hand it to a transport.
let kill = surface.kill_switch();  // local, latched, any thread, native stop first
```

Configuration (`Config`, `GrantSpec`, `SpeechConfig`) comes only from the trusted
supervisor. Nothing in it is read from the wire; a claimed wire identity is never
authentication. Operator grants are intersected with what the backend implements.

## Guarantees (tested)

- Every inbound and outbound message is deterministic CBOR validated against the
  generated `surface:` schema; frames over 1 MiB, wrong-direction and out-of-state
  messages are fatal before any effect.
- Operation input/output is validated against the pinned catalog (`src/bindings.json`);
  no URL is fetched.
- Admission order: kill latch, epoch, grant+expiry, direction, resources, args, deadline
  (exclusive, max 60 s ahead), queue/ledger bounds, speech state, backend preflight.
- Write-ahead ledger (`store.rs`): admission/rejection is fsynced before accept and
  effect; terminal result before outcome event. Exact retries replay the stored receipt;
  a different envelope for a used id is `conflict`. Exclusive state-dir ownership.
  Restart turns uncertain pending work into `failed/unavailable` (deduplicated, never
  replayed) and stops native effects. Lost ledger with a surviving identity, or an
  identity mismatch, refuses to open (provision a new surface identity).
- Kill latch is persistent, stops native I/O through `Backend::stop_all` independent of
  the control link, and no wire message can clear it; only `reset_kill` on a
  disconnected surface.
- Grant revocation/expiry fences native I/O and cancels admitted work before the new epoch
  is published; epoch is persisted first.
- Audio success comes only from the backend sink; the core only checks chunk order.

## Qualifications / open gates

- `SemanticBackend` is a semantic renderer (expression + config). It has no audio or
  display hardware and does not advertise `speech.speak_chunk`, `backchannel`, speech
  events or provider sessions. `speech.stop` always answers `not_found` there.
- Not implemented and never advertised: `speech.heard` / heard alignment, `speech.work_*`,
  heard-history, `llm.complete`, `runtime.inbox`, `test.echo`. Qualified native backends
  and the named S-speech gates in `docs/reviews/spec-v0-m1-gates.md` remain open;
  stream/session/tool state and the normalizer (`speech.rs`) are unit/integration tested
  only against `testing::ScriptedBackend`, a recording double, not real audio.
- The socket transport verifies OS peer UID and private filesystem permissions;
  in-process callers are trusted supervisor application code. The stdio fixture is
  test-only, not an authenticated network endpoint.

## Running the shared wire suite

```sh
cargo test --locked -p coglet-surface-rust -p coglet-mock
cargo clippy --locked -p coglet-surface-rust -p coglet-mock --all-targets -- -D warnings
cargo build --locked --release -p coglet-surface-rust -p coglet-mock --bins
conformance/run --junit target/conformance-registered.xml
```

The registry claims only the surface-under-test side (`peer_roles = ["runtime"]`):
all nine applicable cases run on reference and mock via the framed stdio adapter
and again through the actual in-process queue. The two runtime-under-test version
cases are explicitly skipped, not counted as surface passes. Real AF_UNIX peer
credentials, fragmented negotiation, effect, fatal-frame disconnect and a wrong
UID are additionally tested against `Surface` in `tests/socket_surface.rs`.
The C ABI wrapper's native callback probes do not qualify a real runtime ABI.

Native backends must check `ActionRequest.kill_fence` under the same device lock
as `stop_all` at the actual effect-start boundary. `id` is an opaque native
completion/cancellation token; `wire_id` is the caller's owner-scoped action ID.
Retain the immutable `Sink` when starting capture/provider work; never fetch the
latest binding inside a late callback. Grant renewal invalidates old senders.

A durable running marker is installed before negotiation. An unclean restart
stays stopped even if the kill file failed to persist; repairing storage and an
explicit local reset/fresh negotiation are required. Runtime pruning frees expired
receipts without discarding pending work. Deleting the entire state directory
loses the identity anchor too: supervisors MUST preserve it externally and must
not recreate a previously enrolled identity with a fresh directory. The SDK can
detect partial ledger loss, not destruction of all supervisor state.

Release fixture adapters emit `COGLET_CONFORMANCE_READY` on stderr after durable
startup. The registry harness waits under the existing connection/case watchdogs
before starting the unchanged 200ms first-frame deadline. Release optimization
alone previously left initialization charged to that protocol deadline. The
`surface-readiness` CI job proves the budget separation with 50 normal iterations
and paired slow-initial-fdatasync counterfactual/fixed runs; this does not attribute
the historical CI stalls to a particular storage or scheduler cause.
