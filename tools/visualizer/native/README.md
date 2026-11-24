# Native simulated-mode helper (WIP)

This directory adds an **offline** WAMR verifier/fork engine. It is not yet wired
into the HTTP server or browser and is not full simulated-mode acceptance.
Do not merge the draft PR based on these command-line tests alone.

```sh
# From repository root. The standalone workspace does not modify root Cargo.
CARGO_TARGET_DIR="$PWD/target" cargo build --release --locked \
  --manifest-path tools/visualizer/native/Cargo.toml
COGLET_VISUALIZER_NATIVE="$PWD/target/release/coglet-visualizer-native" \
  uv run --project tools/visualizer --locked pytest tools/visualizer/native/tests -q
```

`fixtures/m2-gridbot-trace.json.gz` is a complete canonical ABI trace, not the
records-only display projection in the viewer tests. Its compressed and expanded
SHA-256, producer revision, proof hash and final state hash are in
`fixtures/provenance.json`. The fixture contains synthetic M2 test data, not a
user archive. Tests verify both file hashes before using the actual trace.

The source capture ran the unmodified `sim/check-native.sh`, then unmodified
`sim/tests/test_native.py` with an operator-selected proof wrapper. At the normal
proof invocation, after child-cogletd termination and before test cleanup, the
wrapper copied `setup.json` and the stopped test WAL to a private directory,
invoked the real `coglet-sandbox-proof` unchanged with the original arguments,
and retained its actual report. Both Gridbot and Cartpole tests passed. Only
Gridbot is committed here. This helper's `export` opened that stopped test WAL
and used `RuntimeService::export_trace` to preserve the complete initial state,
pending state, manifest/code blobs, records and trailing boundary. No live
router WAL or database was scraped. Hashes bind bytes, not producer signatures,
cutoff authentication, or admission/transform qualification.

## Operator commands

```sh
# Explicit STOPPED TEST CAPTURE only. New private output; no overwrite.
target/release/coglet-visualizer-native export SETUP_JSON STOPPED_TEST_WAL NEW_TRACE
# Independent baseline; request JSON is stdin, projected result JSON is stdout.
printf '{}\n' | target/release/coglet-visualizer-native replay TRACE_JSON
# Edit the first successful action receipt's result object, leaving identity,
# status, correlation and every other external result unchanged.
printf '{"receipt_index":0,"result":{"what_if":true}}\n' | \
  target/release/coglet-visualizer-native replay TRACE_JSON
```

The exporter opens a stopped test WAL through the runtime recovery API; it is not
an HTTP endpoint and must never be used against a running producer. `NEW_TRACE`
is created mode 0600 with no clobber on Unix. Publish it atomically yourself only
after the command succeeds. Keep the parent directory private.

## Execution and qualification

1. Validate the complete canonical trace with `Runtime::import_trace`. Its
   observed-transition reconstruction is **not** native verification.
2. Decode the pinned manifest blob; construct a fresh runtime from every required
   initial/pending field and a fresh `NativeEngine` from the pinned executable.
3. Independently execute baseline with `Runtime::replay`, comparing every record
   (except nondeterministic wall duration) and the final output hash. Never pass
   archived calls, returns, usage or output snapshots into an execution engine.
4. A fork starts another fresh runtime/engine and uses `Runtime::step`, not the
   imported receiver. Each generated `record_hash` is the next `previous` link.
   The selected successful `action.outcome.result` is edited as an external
   input. Each later inbox mutation appends **only its original suffix** to the
   current fork inbox, preserving the edit and runtime validation. Non-inbox
   mutations must retain their exact before bytes.
5. Require generated actions, LLM requests, timers, status and failure to match
   the recorded continuation. Otherwise refuse it as inapplicable: no live
   effects/providers, reply fabrication, identity remapping or state replacement.
6. Return only `/data`, events and step/hash metadata, plus first divergent state
   index and final hash. No `/code`, blobs or full input state reaches the view.

This narrow what-if edits **recorded external receipt results**, not arbitrary
observations or guest code. The M2 pilot is stateless and ignores receipt results;
its native policy still executes. Divergence is the genuine runtime inbox/tree
state difference, not a claim that its controller chooses a different action or
that a world simulation was rerun. The existing world-model panel covers action
edits separately. WASI remains unqualified and refused by `NativeEngine`.

Input files are regular/no-follow on Unix and bounded to 128 MiB before reading;
requests are 64 KiB, recursively duplicate-free, integer-only JSON. These are
CLI bounds, **not** a server process sandbox. HTTP integration still needs a
startup-selected source, fixed prebuilt executable, independent CPU/memory/time/
concurrency/output bounds, cancellation and late-response fencing. Neither a
Cargo build in a request nor an arbitrary browser-supplied filesystem path is
acceptable. Native tests currently run only via the explicit command above;
required hosted native/browser CI still needs integration.
