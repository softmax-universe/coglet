# Strict STANDARD recorded replay/fork increment — NOT stage2 PASS

## Implementation and independent review

Existing Unforged `CaosClient` STANDARD jobs now run the unchanged static Coglet
filesystem proof driver inside the already exercised strict inner
`caos-sandbox-exec --mode namespace --net proxy-only`. No proxy socket, fd3
preservation, live secrets or resolver/base mounts are supplied to the inner job.
Outer publication uses existing `caos put STATE /cas/out` and a separate
`/cas/out-trace`; ABI files are neither projected nor augmented with diagnostics.
The unchanged driver removes `proof-input.json` before publishing its real state.

Executable increment `f59b2fe68e489d7312d5a9b921a71970b5ae74f1` received separate
incremental approval `sa-0-1e81ab03` after two independent change-request rounds
(`sa-0-4eeb3af2`, `sa-0-688d604e`). This is NOT final stage2 review. Follow-up serial
batch/independent-verifier conveniences at `bfc4dbe`, `90ecb39` have not received
final review. Hosted bridge was rebuilt before the reviewed-source executions.

Bridge SHA256 `1dc1ba3ce0cb7c93b88d762e5bc8d6b7f7edcaa21deac8e0ebb0ce9b6206b06a`.
Unchanged static driver SHA256
`3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40`.
Consume Unforged `f9c627bcc09346b2923583037a10ac9a2fa8ea12`, no primitive or
deployment changes. Endpoint pin
`e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`;
worker image `native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5`.
STANDARD direct transport does not consume/qualify a resolver graph; do not invent
one or relabel the separate native driver's resolver evidence as this route.

## Actual executions

- Reviewed model step0 request `10b8bd2295d001c7283b1bad9f265cfb96577665`, tree
  `91156afc257cd101d3c6efa2d31b6ed608bc2e54d801fb1933fdcd2c25b1be38`.
- Reviewed model step1 request `26b59a8e6685aeb9a572e0902f69e0b1f19958b8`, tree
  `4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad`.
- All nine Gridbot replay steps in `hosted-standard-gridbot/replay-*` have real
  fresh miss/image observations and complete raw returned trees matching their
  genuine local native recording. Final request
  `5408a8c31398d8573049e0dbd6fa99bec8d0f535`, tree
  `56cdcb744bea9fd625b4a961c434da1082b387b37a60abfcb803f8b239fce9d9`.
- All nine independently recorded fork steps: fork0 in original batch, fork1–8
  in `hosted-standard-fork-tail`. Final request
  `e98f14290cdad09ab618f5b6f5a645c6604d6d69`, tree
  `70327de13e51ba416e868b12221cd5f3c1ce2f72ac1b1e5b32bc477f3c0ec338`.

Every successful invocation reads authenticated completed status, exact raw
sidecar, and every raw output object after execution. CAOS wire objects are
SHA-1; the independent verifier hashes the raw bytes and recursively derives
SHA-256 trees including empty directories. It refuses unsupported modes,
duplicate names, missing objects, noncanonical order and unreachable extras.
Submitted worker/driver/state SHA-1 identities are checked against retained local
bytes, not self-reported tree strings. `verify_standard_batch.py` rechecked the
10 original completed transcripts and eight tail transcripts independently.

The pre-exec shell's namespace/env/fd observations are consumed through an outer
pipe into files absent from inner mounts; the subsequent driver cannot overwrite
previously consumed bytes. These observations are not universal no-effects
proof. There were no canaries or invented zero-call counters. Existing StepHost
records model/action requests without dispatching providers/adapters; recorded
requests are not live calls. The model guest does not inspect reply contents.

Preserved honest failures/limitations:
- First model0 success predates final review/outer-pipe followup, retained separately
  as `hosted-standard-model-0`; never relabel it as reviewed source.
- A model1 command mistyped the operator endpoint. Bridge refused before network
  uploads; retained `hosted-standard-reviewed-model-1` has exit1 and no receipt.
  Correct independently pinned retry is `...-model-1-pinned` above.
- Original serial batch exceeded the tool's 300s timeout after replay0–8/fork0;
  fork1's incomplete directory has no receipt/result and is NOT counted. No local
  batch/bridge processes remained. Tail was explicitly run with a new output
  directory and `--path fork --start-step 1`, finishing all eight remaining steps.
- `server_confirmed_nocache` still means requested nocache plus an observed
  X-Caos-Cache miss, not universal server-side cache bypass. Both Rust and Python
  now refuse absent/false confirmation, per independent review's reproduced
  synthetic admission failure. A fresh salt/execution observation qualifies only
  that invocation, not all future cache contexts.

## Reproduction (from task workspace)

```sh
cargo +1.100.0 build --locked --offline --manifest-path repo/tests/unforged_proof/bridge/Cargo.toml --target-dir unforged/target -j2
repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof record-model "$PWD/repo" > model-recording.json
python3 repo/tests/unforged_proof/record_oracles.py --driver repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --output standard-oracles
```

For each model step0/1 use broker-wrapped `standard_probe.py` with
`--synthetic-transport-only --oracle model --step N --trace model-recording.json
--driver <static-driver> --bridge <bridge> --output <NEW-dir>
--expected-endpoint <pin-above> --expected-image <pin-above>`.

Replay/fork command:
```sh
token-broker exec --scope caos.prod.connect --reason 'Synthetic strict recorded replay and fork' -- python3 repo/tests/unforged_proof/standard_batch.py --synthetic-transport-only --driver repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --bridge unforged/target/debug/coglet-hosted-bridge --oracles standard-oracles --output NEW-BATCH --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5
python3 repo/tests/unforged_proof/verify_standard_batch.py --batch NEW-BATCH --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5
```

Allow >10min for a full batch; persist intermediate child output. Existing result
files must not be overwritten. Explicit `--path` / `--start-step` selects only a
new partial batch, never counts omitted steps as completed.

## Durable evidence and remaining acceptance

`evidence/hosted-standard-interim.tar.xz`, 286704 bytes, SHA256
`3ab16fe97bb88929223b8b73244608f23f4b5ccb5171e288009562cd91536cc3`,
retains original model recording, replay/fork oracles, fork request, raw remote
transcripts/sidecars/objects, exact command/source/binary pins, partial failures,
results and independent checks. Reproducible local state/prefix copies and the
redundant source trace are omitted. Solid XZ preserves byte-exact raw transcripts
while avoiding repeated multi-MiB Wasm snapshots in independently compressed ZIP
members; no object bytes or receipt fields are normalized. Extract with
`tar -xJf <archive> -C <new-evidence-dir>` and rerun the independent verifier.

51fast tests pass normal/-O, eight explicit built-bridge CLI refusals pass, six
explicit native runtime checks pass. Original95files453definitions retained.
Four new state/tree tests ran RED missing module then GREEN; freshness control
ran RED missing API then GREEN. Wire-tree and shell tests were added after their
implementations, not claimed test-first.

NOT stage2 PASS/review-ready: complete initial grant before private serialization
and per-write/send checks, actual factory evaluate/signing/nonce acceptance,
safe live-effect enforcement canaries, actual image/resolver invalidation,
seven named hosted qualification tests, final independent exact-head review,
required CI, native queue/main merge and containing-main proof remain. No PR,
new capacity, paid services, device Git, effect rewrite or stage3 implementation.
Next use this actual full STANDARD driver seam for complete authorized transport
and factory integration; do not return to diagnostic-only discovery probes.
