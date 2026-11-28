# Guarded STANDARD export increment — NOT stage2 PASS

Exact executable/source increment `476ae3ad6fdf91deca70059b83ce04b3bd9e8682`
received independent incremental APPROVE from `sa-0-75dd18f6`. Prior reviews
`sa-0-c0292c39`, `sa-0-35702fb8` requested concrete changes, now covered by the
strong retained-byte verifier and actual Rust-to-Python offline contract. This is
not final stage2 review and authorizes no real private/device data.

## Actual boundary

`guarded-step --synthetic-transport-only ENDPOINT IMAGE SALT DRIVER STATE` retains
immutable source bytes, describes every path/mode/raw leaf hash (including empty
directories), and waits for an operator approval before private wire serialization.
It uses existing Unforged object encoders and public request transport, never a
second scheduler/store/premerge. Includes driver, worker, entire state/reply/prefix,
salt, image, explicit native pool, absent engine, actual base and bundle derivation.
The recipe is a derivation description, NOT a fabricated Git tree OID.

`RecipeAuthority` reuses Gate `TrustKey`/`ArtifactStore` in Python, never passes its
key into the bridge/guest, and rechecks its current record before each approval.
Strong `ApprovalSession` independently validates raw retained leaves before the
first ack, then validates ordered one-shot serializations, exact wire object IDs,
HEAD/optional POST pairs and body hashes, exact submission body, returned
request/execution identity and the status/sidecar/whole-output object graph.
The bridge waits separately before EVERY request, including HEAD, POST, /run and
readbacks. It does not use high-level upload/submit methods with hidden sends.
Revocation prevents the next crossing; it cannot retract prior disclosure/effects.

The CLI is explicitly synthetic-only, issues an ephemeral operator-owned testbed
key, and consumes an already prepared public fixture state. It does NOT claim the
prior state's preparation was private-authorized. End-to-end initial private
preparation and a separately configured durable operator authority remain to wire
before private/factory qualification. Existing synthetic-step/probe behavior remains.

## Tests and corrections

- 67 fast Python tests PASS normal and `-O`; previous51 retained, 16 new.
- Five Rust guarded unit tests PASS; one additional ignored contract fixture is
  explicitly invoked by `cross_language_checks.py` (not silently counted skipped).
- Actual offline Rust `run()` emits two complete transcripts against its persistent
  fake server. Independent Python recipe and strong verifier consume every event
  through `finish()`: PASS2runs, object POST counts `[13,0]`, including nested files,
  empty directories, worker bytes and HEAD hit/miss branches. This is fake-network
  contract coverage, not hosted acceptance.
- Existing8builtBridge/6native checks retained, not rerun this increment.
- Baseline95files453source definitions retained unchanged.
- Parent substantive RED: recipe used an abstract base rather than actual submit
  base; fixed to `CaosBaseSpec::Unforged.base_type_and_value()`.
- Python initial6tests ran RED missing module, thenGREEN. Added invalid method/path
  and malformed-row process-leak controls actually failed RED, then fixed.
- Malformed rows/any exception kill and reap the child; BaseException propagates
  after cleanup. Exact sequence/closed messages and timeouts fail closed.

## Real reviewed-source hosted result

Bridge SHA256 `d901e7f1066cabcb12f75f5a5879f7e4f2dac1f7f4165a3e2cbe8a9526788733`.
Static filesystem driver unchanged SHA256
`3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40`.
Unforged source remains `f9c627bcc09346b2923583037a10ac9a2fa8ea12`.
Endpoint pin `e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`.
Image `native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5`.

`guarded-model-reviewed` actually ran genuine recorded model step1 inside existing
strict namespace/proxy-only. Request `9e669606147dc0b1f7b5f1323343c63e9dc78e66`,
whole ABI tree `4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad`
equals recorded step. 51 approvals =1initial+16serialization+34requests. Authenticated
cache miss/exact image, sidecar `01a9d85bd21745ad6d4a4b87d1a816e93b2a5088`,
separate completed-status/raw-object readbacks and independent existing
`verify_standard_batch.py` verification PASS1. Outer/inner namespaces distinct,
CAOS_SERVER_URLunset/fd3closed, exit0. Still not complete no-effects/canary proof.

Negatives actually exercised:
- `guarded_probe.py --deny-export` against the same real retained recording:
  deniedtrue/bridge_startedfalse/private_staging_writes0; no output directory/job.
- `guarded-model-revoked --revoke-at-sequence 10`: new salt HEAD completed, operator
  revocation before first POST; approvals9, refused missing/revoked, child reaped
  returncode-9, no submit/execution. Prior authorized metadata HEADs cannot be undone.
- Rust offline denies at every serialization/send boundary; Python malformed-row,
  method/body/order/mutation/readback/callback/EOF/cleanup tests remain offline.

## Reproduce from task workspace

```
python3 repo/tests/unforged_proof/cross_language_checks.py --target-dir unforged/target
python3 -m unittest discover -s repo/tests/unforged_proof -q
```

Hosted command (broker auto scope `caos.prod.connect`, envonly, scopes/pending
checked; no tickets persisted):
```
token-broker exec --scope caos.prod.connect --reason 'Reviewed synthetic operator grant and recorded model step' -- python3 repo/tests/unforged_proof/guarded_probe.py --synthetic-transport-only --oracle model --step 1 --trace model-recording.json --state hosted-standard-reviewed-model-1-pinned/state --driver repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --bridge unforged/target/debug/coglet-hosted-bridge --output NEW-GUARDED-DIR --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-bridge-sha256 d901e7f1066cabcb12f75f5a5879f7e4f2dac1f7f4165a3e2cbe8a9526788733
```
Add `--deny-export` (no broker needed) or `--revoke-at-sequence 10` for negatives.

Durable exact raw evidence `evidence/hosted-guarded-interim.tar.xz`,6168bytes,
SHA256 `1a9b62250228d362f0703bdba73a8376cd677483f51eaf2a9018258ac88fff89`,
contains model recording, complete actual transcript/scope/commands/results,
revocation transcript and independent verification. No keys, tickets or private
staging copies. Prior20strict model/Gridbot steps remain separate prior evidence;
this run did not rerun completed batches or relabel them as guarded.

## Still required

End-to-end initial private preparation under complete authority; operator-configured
real gate `evaluate_hosted` before signing and one-use nonce acceptance; safe genuine
no-effects enforcement canaries; actual resolver/image changes and warm acceptance
invalidation; seven named hosted suite; final exact-head independent review,
required CI/native queue/main merge and containing-main hosted proof. No PR,
new capacity/deployment, GPU/paid service, device Git or stage3. NOT stage2PASS.
