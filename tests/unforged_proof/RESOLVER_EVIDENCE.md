# Actual retained-client resolver revision controls — NOT stage2 PASS

## Source and trust boundary

Plan independently approved by separate session `sa-0-ef126bcc`. Initial
implementation5558153dd660651db4e8a87e5c79eaa86459f856 independently approved by
`sa-0-b281c9c3`. Executed followup source
80bff7241c6623adc9e67b87391a97537a4796ce independently approved by `sa-0-3b15b303`.
These are incremental reviews, **not final stage2 exact-head approval**.
Offline verifier1460cdd independently approved by `sa-0-d8271ef8`, which reran
all5observations/32objects and rejected three independent corrupted-graph probes.
The verifier's extra inner pins are recorded in the content-hashed archive,
not separately authenticated server fields; offline integrity is not fresh auth.
Archive `evidence/hosted-resolver-interim.tar.xz`:3704416bytes, SHA256
f34d4962776702ff6a48701838cd07767f22c52ed3ca8359048501b2431adc39.
Unforged remains f9c627bcc09346b2923583037a10ac9a2fa8ea12, unmodified.

This command consumes the existing native CaosDriverRunner, not STANDARD or a new
resolver/executor. ONE runner owns ONE CaosClient across all five calls. Actual
resolver B is a new ordinary Git commit/tree containing every original A entry
unchanged plus exactly one regular synthetic metadata blob. Its content is the
fixed `coglet-stage2-resolver-control-v1` header and fresh random hex revision.
Both Rust and Python validate this exact delta; strategy/engine/driver unchanged.
No worker was deployed, rotated or provisioned. This is **not image-upgrade proof**.

The native high-level transport remains **synthetic-only**, because it has not
been placed behind the separate complete guarded STANDARD privacy seam. The
caller verifies the known synthetic model oracle, genuine native deterministic
record validation, exact prefix/current-input payload and every prior snapshot
file, and actual program driver bytes before upload. No device/private data or
provider call was authorized. Existing prepared/guarded/factory evidence remains
separate; this run does not upgrade its authority or historical source pins.

## Actual results

`hosted-resolver-reviewed/command.json` retains full pre-execution pins and exact
bridge command; original trace and entire freshly rerecorded trace are retained
separately (no telemetry/hash-chain rewriting). Actual raw trusted-client receipts
and commits were immediately read back with Git replacements disabled.

- Resolver A b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f
  tree366b27d9d2d256bb42366fc4b6852deeb31ec9f6c788f127eb4dc2a9c43ce203.
- Resolver B cc6f161bec5be7e29d285f8a90bc4e517d6826982c09c9b86c85ae4b89cf2d00
  tree99b7391e027605b467ac5c06d7b4b52e3bbfebde43d2e81449e3f63a6c35ab8a.
- Unchanged static driver SHA256
  3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40.
  This is the earlier real-model driver, **not the later canary driver2528aa...**.
- Bridge SHA2562589a1a53d3d7fa38e6e16638cd92c28b9874f6c71cef883b52dcd15fc79b125.
- Authenticated endpoint identity
  e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce.
- Every receipt reports actual image
  native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5.
- Every complete output tree equals the genuine native model step1 tree
  4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad.

| Observation | Actual disposition | Returned commit |
|---|---|---|
| A cold | executed=true/cache=none | 724b47e808d2d16963e9663c42b4d7cf5927614250517b85e8fd40a28a9dfd48 |
| A warm | executed=false/cache=caos | 46b47206907d5445d3fd33a32b80f6bced7db9bb21b054494cc8630667502cee |
| B first default | executed=true/cache=none | 443d1e848d2d20f65891d85cc34767b0044e775697d2123c6c40cb8d7af899d5 |
| B warm | executed=false/cache=caos | 57c5aa2f48c4979ff6021ab358268c33812d8df09b3f10447126646e73ffb6b5 |
| A return | executed=false/cache=caos | 7187ed00b1744309874e4dafa08c4ddeba98e8768929952b79907ed5d1f83811 |

Programmatic aggregate:5observations/5unique commits/2executions/3remote hits.
This establishes actual changed-resolver invalidation of the remote warm result,
not merely changed expected-pin rejection. It does not establish a local memo hit:
base-only discovery may return no digest and disable local prelookup by design.
No universal no-write-cache/bypass API or deployed-source attestation is claimed.

## Reproduce

Use task-private Git>=2.45 with reftable; normal system Git2.43 cannot read these
stores. Build the opt-in bridge using existing locked Cargo toolchain1.100.
Supply an existing native synthetic-model probe's disposable OS and actual trace
(the earlier hosted-model-1 + prepared-model/trace.json used here). Prior object
stores are not guaranteed on another host; regenerate with the existing native
model preparation/probe commands when unavailable. Never substitute an empty or
fabricated OS. Old completed batches need not be rerun merely to verify evidence.

```
token-broker exec --scope caos.prod.connect --reason 'Coglet resolver controls on existing hosted runtime' -- \
  python3 tests/unforged_proof/resolver_probe.py \
  --git /absolute/task-private/git --synthetic-transport-only \
  --probe /absolute/hosted-model-1 --trace /absolute/prepared-model/trace.json \
  --driver /absolute/static/coglet-unforged-proof --bridge /absolute/coglet-hosted-bridge \
  --output /absolute/NEW-hosted-resolver-probe \
  --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f \
  --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 \
  --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce \
  --expected-driver-sha256 3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40 \
  --expected-bridge-sha256 2589a1a53d3d7fa38e6e16638cd92c28b9874f6c71cef883b52dcd15fc79b125
```

The bridge command and complete retained raw32object graph are included in the
compact archive; local Git OS and reproducible prefix copies are omitted. Offline
verification needs no ticket or live calls:

```
uv run --project sim --locked python tests/unforged_proof/verify_resolver_probe.py \
 --batch /absolute/extracted/hosted-resolver-reviewed \
 --expected-source 80bff7241c6623adc9e67b87391a97537a4796ce \
 --expected-driver 3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40 \
 --expected-bridge 2589a1a53d3d7fa38e6e16638cd92c28b9874f6c71cef883b52dcd15fc79b125 \
 --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 \
 --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce \
 --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f
```

## Tests, negatives and honest failures

107fast tests PASS normal/-O (100retained+7resolver verifier regressions).
11builtBridge CLI refusal tests PASS (8retained+3new). New7fast tests genuinely
RED missing module then GREEN;3bridge tests RED unknown command then GREEN.
CLI orchestration and offline raw-object verifier were **not test-first**.
Existing6native+3canary tests are retained but not rerun in this increment.
Original95file453definition inventory is retained; no original test removed.

Offline verifier rechecked all5actual receipts/commits, exact A/B delta and all32
raw Git objects/links/full output roots. Additional15LOCAL mutation negatives
refused:missing/ambiguous receipts, resolver substitution, missing/extra run,
firstBhit, each warm miss, wrong full tree, changed expected image/resolver/source/
driver/bridge. These are local mutations **not actual worker upgrades**.

Two shell env/uv forms were guard-refused before execution. Supported direct
absolute-path command with explicit application --git option executed normally;
no guard/approval configuration changed. First actual local preparation failed
closed before output/hosted I/O because comparing whole freshly recorded archives
included measured wall-time and derived hash-chain fields. Source80bff72 fixes
that by reusing native record()'s existing deterministic verification, retaining
both unmodified full archives and comparing all full output hashes. No failed
hosted job or invented receipt was counted. No prior hosted batch was rerun.

Still outstanding: actual retained-client image-change qualification, complete
freshness scope and seven named real hosted qualification tests, final independent
exact-head review, required CI, native queue/main merge, containing-main hosted
proof and final evidence attachment/review handoff. This is **NOT stage2 PASS**.
