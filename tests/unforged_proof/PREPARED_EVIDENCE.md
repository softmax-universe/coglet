# Held-source preparation increment — NOT stage2 PASS

Source `1cd62a763b1b5be1abc4fc662909cbcd4e6a2610` contains the reviewed synthetic
prepared probe. Independent review `sa-0-151010a1` APPROVED that exact head.
Underlying `guarded_preparation.py` was independently APPROVED at
`a26ce5f65ce412e450dc0f83d04749413cb8356b` by `sa-0-0042e5d1` after prior
reviewers requested cleanup and malformed-source corrections. Neither is final
stage2 review, production/private-device authorization, or gate qualification.

## Boundary and limitations

`Sources` consumes already-held immutable trace/snapshot/driver/worker bytes and
operator-selected target, sender, authenticated recipient, step, salt and image.
Existing `ExportAuthority` authenticates actual source-blob descriptors (incremental
Git blob SHA256, not imaginary tree IDs) plus the retained public selection/role
binding BEFORE decoding or private payload serialization. Held source acquisition
is outside this seam; this does not authorize fetching/reading a device snapshot.
The CLI native-validates an exact private copy of the held trace after the initial
source grant. Missing recorded replies refuse before native execution or staging.

`prepare` reuses complete model reply closure, prefix input and snapshot helpers.
It retains exact serialized state bytes, validates prior whole tree and bounds,
and builds the already-reviewed complete STANDARD transport derivation recipe.
A distinct existing `RecipeAuthority` grant is required before any state staging.
Neither API constructs keys or self-issues grants. `Prepared.execute` uses the
same pinned guarded-step bridge/strong verifier; both source and recipe grants
remain live at every write and wire approval. Keys never enter bridge or guest.
The CLI issues ephemeral OPERATOR TESTBED grants, not independently configured
production authority. Its synthetic-only restriction remains mandatory.

Staging uses a new0700 directory, dirfd/no-follow traversal, exclusive0600 files,
and checks before each mkdir/open/write. Retained fd path survives through child
reaping. fd-safe stdlib cleanup closes all descriptors even if removal fails,
retries once, preserves actual caller exceptions and reports residual private
staging explicitly. It cannot undo a genuinely persistent filesystem denial.
An unrelated already-handled exception must never suppress cleanup failure.

## Actual test evidence

- Existing67fast cases retained.16preparation plus3CLI cases added;86PASS normal
  and `python3 -O`. Original95files453source definitions remain unchanged.
- Initial8preparation cases ran RED missing module, thenGREEN. Reviewer regression
  cases genuinelyRED: malformed nonobject trace, callerOSError replacement,
  cleanup fd leak/unexpected descendants, ambient-handler cleanup suppression.
  All fixed.2execute integration tests RED missing method thenGREEN.
- A real LOCAL fake bridge reads staged input, requests strong initial approval,
  then exits: incomplete transcript REFUSES. Not counted as hosted execution.
-3CLI cases were NOT test-first: synthetic ack missing, deny-export before JSON/
  native/bridge/output, complete reply refusal before native tools. They PASS.
- No Rust source/lock changed. Fresh locked builds succeeded on this new host:

```
cargo +1.100.0 build --locked --manifest-path repo/tests/unforged_proof/bridge/Cargo.toml --target-dir unforged/target -j2
RUSTFLAGS='-C target-feature=+crt-static' cargo +1.100.0 build --locked --manifest-path repo/tests/unforged_proof/driver/Cargo.toml --target x86_64-unknown-linux-gnu --target-dir repo/target/static-proof -j2
python3 -m unittest discover -s repo/tests/unforged_proof -q
python3 -O -m unittest discover -s repo/tests/unforged_proof -q
```

## Real reviewed-source hosted run

Unforged remains `f9c627bcc09346b2923583037a10ac9a2fa8ea12`.
Bridge SHA256 `d901e7f1066cabcb12f75f5a5879f7e4f2dac1f7f4165a3e2cbe8a9526788733`;
static driver SHA256 `3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40`.
These rebuilt to exactly the prior binaries. No deployment/capacity change.

New local `record-model` acquisition made a genuine2step NativeEngine recording
and held initial snapshot. Acquisition deliberately precedes selection of export;
its local serialization is NOT falsely described as protected by a later grant.
Actual prepared probe source1cd62a76 executed modelstep0 using BOTH grant seams:
request `c70b18b1fdeddba1413e5d33f6321c4af4541a62`,50approvals, actual cold miss,
exact pinned image, full tree
`91156afc257cd101d3c6efa2d31b6ed608bc2e54d801fb1933fdcd2c25b1be38`
equals recording. Completed status binds raw sidecar
`d5ac8a21b097dd626cd95fb45d01c00baa8046e0`. Existing strict inner namespace/proxy-only
ran with CAOS_SERVER_URLunset/fd3closed; distinct namespaces and exit0 are observed,
NOT complete no-effects/canary qualification. STANDARD consumes no resolver graph.
Requestednocache+observedmiss still is not universal cache-bypass attestation.

Independent offline readback rechecks actual status/request/sidecar Git-blob hash
and whole raw output object graph against the held original trace: PASS1.
`verify_held_prepared.py` initially failed its Python gate import path, fixed and
rerun successfully; not an execution failure. Actual `--deny-export` against the
held real recording returned deniedtrue/native_startedfalse/bridge_startedfalse/
private_staging_writes0 and no output directory. No broker needed for denial.

Reproduce after extracting evidence at the task workspace beside `repo/`:

```
python3 acquire_held_model.py
# broker scopes/pending checked: caos.prod.connect auto, environment-only ticket
token-broker exec --scope caos.prod.connect --reason 'Reviewed held synthetic source grant through guarded STANDARD execution' -- python3 repo/tests/unforged_proof/prepared_probe.py --synthetic-transport-only --oracle model --step 0 --trace held-model-trace.json --snapshot held-model-snapshot.json --coglet coglet:1111111111111111111111111111111111111111111111111111111111111111 --driver repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --bridge unforged/target/debug/coglet-hosted-bridge --output NEW-PREPARED-DIR --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-bridge-sha256 d901e7f1066cabcb12f75f5a5879f7e4f2dac1f7f4165a3e2cbe8a9526788733
python3 verify_held_prepared.py
```

Verifier targets the archived `prepared-model-reviewed` directory; point it at a
new run explicitly if rerunning. CLI command metadata lacks the older
`verify_standard_batch.py` expected_tree convenience field, so this archive uses
the separate verifier comparing directly with the original recording, not a
post-edited raw command artifact. No tickets/keys/temporary authority stores in
archive `evidence/hosted-prepared-interim.tar.xz`,7140bytes,
SHA256 `3b74db312d90af02a220d9f58e7e7fb04854d7b878eca1646a8a81d739adb6e8`.
Prior hosted batches were NOT rerun or relabeled as prepared.

## Still required

Actual operator-configured factory `GateService.evaluate_hosted` before signing,
existing one-use challenge nonce/binding and unused challenge on infrastructure
failure, safe genuine enforcement canaries, actual resolver/image invalidation,
seven named hosted qualification cases, independent final exact-head review,
requiredCI/native queue/main merge/containing-main proof and durable final
attachment. Gate/runtime implementation remains unedited. No stage3, new capacity,
GPU, paid service, device Git, or effect-path rewrite. NOT stage2PASS.
