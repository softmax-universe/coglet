# ABI conformance data

[`cases.json`](cases.json) uses the harness timeline envelope: name and steps,
with send, exact expect (within_ms=200) and journal_hash/path assertions. The
adapter uses u32be-length deterministic CBOR framing by default.

## Adapter fixture profile: abi.step

Each case begins with a fresh runtime fixture; further sends in that case retain
its durable outbox/history. `send.operation=abi.step` carries:

- initial_files: pre-boundary path -> hex snapshot;
- manifest: pinned manifest;
- initial_effect_ids, initial_inbox_wakes, initial_timers, initial_llm_requests:
  on the FIRST send only, seed host state exactly as the ABI trace header defines.
  This is setup data, not expected output. Later sends retain actual evolved state
  and must not reset it. Historical-ID cases seed completed IDs with no pending
  requests; fresh-ID controls use empty ledgers. The adapter must independently
  compute conflict/staged returns, never copy guest_script's expected result;
- inputs: trace/coglet/step/time/seed/previous, engine profile, effective budgets,
  grants, batch and authenticated boundary changes;
- guest_script: ordered calls, ordinary file writes (path -> hex), termination
  (committed or aborted), scripted engine usage, and failure when aborted.

This is a data-only script for the **runtime's engine seam**, not an implementation
or guest-language source file. Calls supply operation, argument and the expected
host result; the adapter must check the host actually returned that result, not
use it to override runtime permission checks. Termination/usage describe the
scripted engine outcome; they are not measurements of a particular Wasm engine.
The real runtime and mock runtime adapters must execute the same script through
their engine seam and produce `{journal, state_hash, record_hash}`. Do not simply
return the expected journal. Engine-loader/guest allocation conformance belongs
to the engine suite and guest SDK cases, not this scripted profile.

Expected journal is the full ABI step-record. state_hash asserts the snapshot;
record_hash asserts the journal contents/chain independently. No-effects, action,
timer, async LLM request, logging, wall and compute abort rollback, inbox boundary,
and two-wake LLM reply scenarios are included. No fixture performs a live LLM call.

Schema/fixture tests check internal consistency, golden encodings and hashes;
these are **not** a claim that a real runtime has passed the black-box harness.
See [the ABI](../../spec/abi.md) and its [fixture coverage](../../spec/fixtures/abi/README.md).
