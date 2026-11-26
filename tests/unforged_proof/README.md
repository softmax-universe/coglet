# Stage-1 unforged proof (opt-in)

This runs the **real counter guest**, in WAMR inside `cogletd`, for five steps.
For each recorded step it submits a separate **Merge Stream** job using the
current CLI, `unforge stream exec`, then checks the **entire output tree** against
the recorded ABI `output_hash`. No subtree projection, hash conversion, mock
engine, or copied expected output is accepted. See ADR-0035 and
[staging](../../docs/cogent-design/unforged-staging.md#later-stages-not-part-of-this-change).

This is a local, synthetic, unsigned proof, **not** a production sandbox,
signature/CAOS qualification, or stage-2 authorization. Run as an ordinary user,
not root. The runner launches native code; its non-root local sandbox is not an
OS security boundary. Do not feed it private or untrusted inputs.

## Prerequisites and run

Linux with Rust, C compiler, CMake, Python 3, and **Git 2.45+ with reftable**.
Put that Git first on PATH for both the harness and all runner subprocesses.
The measured run used **Git 2.48.1** (the worker's default Git 2.43.0 cannot
initialize unforged). Build unforged separately; it is not a Coglet dependency:

```sh
git clone https://github.com/softmax-universe/unforged ../unforged
(cd ../unforged && CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_INCREMENTAL=false \
  cargo build --locked --bin unforge -j2)

# From the Coglet repository root:
rustup target add wasm32-wasip1
cargo build --locked -p coglet-runtime --features native-engine -p coglet-cli -j2
cargo rustc --locked --manifest-path coglets/counter/Cargo.toml \
  --target wasm32-wasip1 --release --lib -- -C link-arg=--max-memory=16777216
cargo build --locked --manifest-path tests/unforged_proof/driver/Cargo.toml \
  --target-dir target -j2
python3 -m unittest discover -s tests/unforged_proof -v
python3 tests/unforged_proof/run.py --unforge ../unforged/target/debug/unforge \
  --output ../unforged-proof-evidence
```

Output must be a **new directory**. `--bin-dir` overrides `target/debug`.
The R3 canonicalization/publication transform is applied by `coglet build`;
the explicit maximum memory is required by native engine admission. No changes
to workspace dependencies, default CI, journal structure, or step semantics.
This directory is deliberately outside the default build; CI can opt in by
running the commands above with an independently pinned unforged checkout.

If Git must be provisioned without sudo, build a release tarball into a private
prefix; prepend `<prefix>/bin` to PATH. This proof needs only local Git, so builds
without OpenSSL/curl/expat/gettext/Tcl/Tk work (`NO_OPENSSL=YesPlease`,
`NO_CURL=YesPlease`, `NO_EXPAT=YesPlease`, `NO_GETTEXT=YesPlease`,
`NO_TCLTK=YesPlease`). Do not replace the host Git.

## What runs and what is checked

1. Build/pin counter using the existing CLI package API. A local supervisor job
   wakes it five times with recorded debug events, step/time/seed/budget/grants.
   `cogletd --sandbox-stdio` executes all five steps **in its own process** and
   returns the normal trace with initial, per-step, and final tree hashes.
2. For step N, import **only the trace prefix before N**, reconstructing prior
   coordinator metadata without invoking a guest. Check the prior snapshot's
   tree independently with real SHA-256 Git. The job input contains that prior
   tree plus the exact recorded step inputs; no current/future output, expected
   tree, guest delta, or future trace blobs are supplied to the job.
3. Merge Stream's program tree contains the actual native proof driver, including
   the same Coglet runtime/WAMR build. It restores the prefix, byte-checks its
   snapshot against the mounted prior state, and calls `Runtime::step` exactly
   once with a fresh `NativeEngine`. It writes the resulting ABI files and
   deletes the input envelope. Unforged snapshots the worktree and strips its
   unchanged program overlay; **that returned root tree** is compared, unchanged.
4. Force execution with cache unavailable, then repeat with the cache available.
   **Current `stream exec` has no no-cache switch.** Instead the harness deletes
   `refs/notes/memoization` immediately before every cold job in its own fresh
   instance and requires `memo_hit=false`. Thus no prior cached result can answer
   that call. Memoization *writes remain enabled*, priming the next call; the
   immediate repeated job must say `memo_hit=true` and match the same root tree.
   This is forced-cold execution by cache eviction, not a no-write-cache API.

The helper uses existing runtime WAL serialization only to inspect the private
snapshot; it adds no public runtime API. Trace-prefix reconstruction replays
observed historical transitions, not historical Wasm; only the one requested
step is independently executed by the unforged job. Identity is **tree identity**,
not commit identity: commit timestamps/provenance differ and are outside stage 1.

`trace.json`, `hot-job.json`, package pins, `results.json`, and each raw cold/warm
receipt remain in the evidence directory. Its `unforged-os/dev` retains the job
objects and provenance notes. Every row is saved before checking PASS; a mismatch
raises nonzero without weakening equality. `test_proof.py` covers unequal cold or
warm trees, unexpected cold hits, missing warm hits, incomplete receipts, prefix
output exclusion, and checks under `python3 -O`.

## Stage-2 findings (source inspection, not hosted verification)

Inspected unforged `fdae8a5`:

- Local `LocalDriverRunner::compute_request_memo_key` (`src/driver.rs`) includes
  resolver id, driver bytes/source, operands/trees, seed and arguments, **not a
  resolved worker image**. This proof embeds the native driver in the program
  tree, but does not prove a complete worker-environment pin.
- `CaosDriverRunner::compute_memo_key` (`src/caos.rs`) includes the configured
  `CaosBaseSpec` kind/value. The read-through cache is consulted before remote
  submission. A mutable locator/docker reference or `Unforged` sentinel is not
  the resolved image; an explicit `Hash` base is pinned. Therefore **no general
  resolved-worker-image guarantee** exists in the local read-through key.
- Local Merge Stream returns `memo_hit`, puts `Memo-Hit` on the result commit and
  `memo_hit` in `refs/notes/provenance`. These distinguish execution vs local
  cache hit in this proof. CAOS `DriverOutcome` does not establish whether the
  remote service itself executed vs served its cache, so this is **not** proof
  of a complete cross-layer `executed` receipt or resolved `worker_image` field.

No stage-2, router-storage, transport, or effect-path work is authorized here.
