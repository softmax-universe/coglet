# Face text-mode guest

Product-neutral deterministic controller using `sdk/guest-rust`. No persona,
model/provider selection, credentials, or raw audio. The baseline echoes a
non-whitespace terminal transcript; it does not call an LLM or emit expression
animations. The manifest requires the baseline speech/expression controls and
exclusive microphone/speaker ownership; grants and capture configuration remain
operator/host responsibilities.

## Build and test

Run from this directory (so Cargo finds the local target configuration):

```sh
cargo test --locked
cargo build --locked --release --target wasm32-wasip1
```

Artifact: `target/wasm32-wasip1/release/coglet_face.wasm`.
The local compiler configuration declares maximum linear memory 16 MiB (256
pages), matching `manifest.json`. This is an independent Cargo workspace.

## Policy and persistence

- `speech.start` opens an utterance; partials advance its revision but never
  produce output. Only an increasing-revision `speech.end` for that currently
  started utterance can respond. Empty/whitespace finals close without output.
- Each response is one `speech.speak_chunk`, chunk zero and final true, on a fresh
  deterministic host-context stream ID distinct from the action ID. Text is an
  unchanged prefix of the transcript, capped at 4096 UTF-8 bytes at a scalar
  boundary. Inputs over the speech protocol's 16384-byte cap fail closed.
- Source sequence fences suppress duplicate/reordered events. Closed captures
  reject later duplicate finals; finals for a superseded utterance are ignored.
  This relies on the specified surface normalization: unique utterance IDs and
  ordered start/partial/end events. It is not an unlimited utterance-ID ledger.
- `speech.barge_in` acknowledges the native reflex without producing replacement
  speech. No output text, pending replay queue, or claim of heard text is retained.
  In duck mode the native stream may continue; the guest does not force stop.
- `/data/face-state.json` contains only per-source sequence/capture fences and the
  last committed host step. At most eight sources and 8192 encoded bytes are
  allowed. Source fences are never evicted: source exhaustion, invalid state or
  oversized state fails closed. Host steps must increase with persisted state;
  a reset step is rejected to prevent effect-ID reuse. Replay must restore the
  archived pre-step state and its original context.
- The runtime must atomically publish staged emits and filesystem changes, rolling
  both back on failure. Native tests use an in-memory transactional test host.

## Evidence boundary

The actual Wasm imports restricted WASI file operations. The production runtime
now hosts these transactionally. The named runtime integration test
`persisted_face_wasm_emits_to_compiled_adapter_barges_reopens_and_reexecutes`
executes this guest against real capture events from the compiled Swift adapter,
asserts its emitted speech is rendered then cancelled at 1200 ms, collects the
adapter's owner-only heard read into journaled `runtime.heard` state containing
only the aligned `Hello` prefix, reopens the WAL, and verifies a duplicate old
final cannot emit again. Independent actual-Wasm
re-execution matches every archived output hash. The simulator owner is explicitly
set to this manifest's ID, not changed in the guest.

Run from the repository root after building the Swift executable and this guest:

```sh
cargo run --locked -p coglet-runtime --features native-engine \
  --example qualify_apple_guest -- coglets/face/target/wasm32-wasip1/release/coglet_face.wasm
FACE_MAC_EXECUTABLE="$PWD/surfaces/face-mac/.build/debug/face-mac" \
COGLET_FACE_WASM="$PWD/coglets/face/target/wasm32-wasip1/release/coglet_face.wasm" \
  cargo test --release --locked -p coglet-runtime --features native-engine \
  --test face_mac_adapter -- --ignored
```

Release host execution is intentional: debug WAMR host bookkeeping exhausted the
unchanged one-second guest wall budget. No production deadline was enlarged.
This qualifies injected-media RuntimeService heard projection/replay, **not** actual
C-ABI embedding, native alignment, acoustic measurement or full M2b acceptance. macOS
execution of the new integration must pass in the Swift workflow before claiming
native proof.
