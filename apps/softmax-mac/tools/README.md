# Installed embedded runtime configuration

`install_speech_echo.py` compiles the real Rust guest, applies the existing R3
NaN-canonicalization publication transform, and installs **operator-owned**
configuration. It does not install host-side actions or a mock guest script.
No model/provider is involved. Existing `coglets/echo` is unchanged.

```sh
python3 apps/softmax-mac/tools/install_speech_echo.py \
  --output-dir "$HOME/Library/Application Support/Softmax/runtime" \
  --wal "$HOME/Library/Application Support/Softmax/runtime/state.wal"
```

Requires Rust's `wasm32-wasip1` target. Build the embedding library with
`native-engine` and unwinding Rust panics (the default), **not** `panic=abort`.
The generator refuses an existing runtime.json or WAL instead of resetting trace,
seed or identity. Retain the installed config across app launches; never generate
fresh IDs/seeds on each start. The output directory is private (0700); JSON files
are 0600. WAL's parent must exist and be private/writable by the installed app.

## Swift host contract

The host reads these installed files; no source-checkout paths are needed at
runtime:

1. `binding.json` is the exact UTF-8 JSON buffer passed to
   `coglet_runtime_configure`. The immutable binding must match the surface hello
   identity and grant owner. Repeating identical configure is idempotent.
2. `runtime.json` is the exact UTF-8 JSON buffer passed to
   `coglet_runtime_start` **before** opening `EmbeddedTransport`. This contains
   the complete Wasm and provenance as hex; `step.wasm` is retained as an audit
   artifact, not read by the C API. Native schedule is required for execution.
3. Close every transport link before `coglet_runtime_stop`. Result 1 means a link
   still owns the service; 0 means ownership was released.

Exact output shape (all setup arrays are empty, never canned host output):

```json
{
  "binding.json": {
    "surface": "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "permitted_coglets": ["coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
  },
  "runtime.json": {
    "wal": "/absolute/private/writable/state.wal",
    "setup": {
      "manifest": {
        "version": "1.0",
        "id": "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "subscriptions": ["speech.end"],
        "required_capabilities": ["speech.speak_chunk"],
        "exclusive_outputs": ["audio.mic", "audio.speaker"],
        "budgets": {
          "compute": 100000000,
          "compute_mode": "wamr-2.4.3-fast-cumulative-v1",
          "memory_pages": 256,
          "wall_time_us": 5000000,
          "max_events": 16,
          "host_bytes": 1000000
        }
      },
      "initial_files": {
        "/code/bin/step.wasm": "<actual compiled and R3-rewritten Wasm hex>",
        "/code/build/provenance.cbor": "<actual deterministic hub-build-record CBOR hex>"
      },
      "initial_effect_ids": {"actions": [], "timers": [], "llm_requests": []},
      "initial_actions": [],
      "initial_timers": [],
      "initial_llm_requests": [],
      "initial_inbox_wakes": []
    },
    "native_schedule": {
      "trace_id": "<fresh lowercase 32-hex ID, retained across restart>",
      "seed": 7,
      "runtime_bytes": 33554432,
      "stack_bytes": 262144
    }
  }
}
```

The displayed wrapper is illustrative: **there are two separate JSON files**,
not a wrapper document to pass to Rust. `runtime-config.schema.json` describes
the start buffer. The generator samples the actual u64 seed once at install.
`build.json` contains the provenance record: compiler output and rewritten hashes,
actual checkout HEAD, command recipe hash, ADR policy file hash and R3 transform
source hash. This is a local fixture build record, **not** signed hub publication
or a claim the checkout is clean. Production release admission/signing remains
with the hub publication pipeline.

The surface must advertise/grant `speech.start`, `speech.end`,
`speech.speak_chunk` and common-owner `audio.mic`/`audio.speaker` leases, with the
frozen speech schema URIs. The guest subscribes to final transcripts only, emits
one final chunk from actual input text, ignores empty finals, and truncates long
text at a UTF-8 boundary to the frozen 4096-byte speak limit. The surface still
owns native recognition/playback and must not report success before rendered
playback. Deterministic stream IDs are trace-local; keep the WAL/config together
and do not reset them while a surface retains old stream tombstones.

## Recoverable failures and real C-pointer smoke

All exported C functions catch unwinding Rust panics. A caught panic returns -2
(where a return value exists) and fences the **whole runtime generation**. No
fenced link may send, receive queued output, or execute again. Close all links,
then stop discards poisoned service state; restart reopens the WAL. Stop never
clears the poisoned service's mutex for reuse. Unknown durable attempt outcomes
retain the existing runtime reconciliation fence; reopening is not permission to
blindly retry an uncertain guest attempt. A guest trap is instead recorded as an
aborted step, consumes its batch once, and stages no effects.

This is in-process recovery only. Fatal abort, SIGSEGV, OOM, invalid native
pointers, a panicking panic hook and native memory corruption are NOT isolated.
A throwing panic-payload destructor is not run across C (the fault path leaks
that payload). Product builds must not enable the fault-injection feature.

```sh
cargo test -p coglet-runtime --features native-engine,ffi-test-hooks --locked -- --test-threads=1
cargo build -p coglet-runtime --features native-engine,ffi-test-hooks --locked
# Use libcoglet_runtime.dylib on macOS, .so on Linux.
python3 apps/softmax-mac/tools/smoke_embedded_runtime.py \
  --library target/debug/libcoglet_runtime.so --installed /fresh/install --panic
```

`--panic` calls the test-only `coglet_runtime_test_panic()` C symbol. The injection
holds the ownership and service mutexes; the smoke verifies -2, blocked links,
stop=1 until link close, successful stop/reopen, actual compiled `speech.end` →
`speech.speak_chunk`, and checksum-checked durable WAL records. Normal builds do
not export that symbol.

For the separate real guest-trap test, install into another fresh directory/WAL
with `--trap-injection`, then run the smoke with `--expect-trap` (optionally also
`--panic`). It verifies an aborted real compiled step with positive compute,
zero actions, unchanged input/output state hashes, no act dispatched and no
reexecution after reopen. Never package the trap-injection guest as a product
fixture. These scripts qualify the runtime pointer/guest boundary on the machine
where run, not Apple native UI, microphone, acoustic playback or hardware timing.
