# Stage 2 increment: hosted capability discovered, NOT qualification PASS

Latest continuation12: see [FACTORY_EVIDENCE.md](FACTORY_EVIDENCE.md). Actual
operator-configured GateService.evaluate_hosted now precedes signing: native
behavioral suite acquires complete existing RuntimeService archive, every hosted
step is fresh salt-bound to full challenge+index+operator attempt and compared to
that exact oracle before signing. Reviewed8e5b0d2/a6eec01/9fcd765 increments.
Actual native3step archive compatibility PASS; actual hosted factory behavioral
PASS9steps and honest behavioral rejection3steps; per-step raw status/sidecar/full
ABI graphs independently reverified. Existing one-use/fixture-only/production
refusals retained, raw signed artifact retained for PASS9.96fast normal/-OPASS;
unconfigured sim24PASS/5native SKIP; configured retained-native regression wrapper
timeout180s, NOT a pass; child confirmed gone. No old hosted batches rerun.
Genuine canaries, actual resolver/image invalidation, seven named hosted suite,
final independent review/required CI/native queue/main/containing-main remain.

Previous continuation11: see [PREPARED_EVIDENCE.md](PREPARED_EVIDENCE.md). Reviewed
1cd62a76 connects initial held-source grant before decode/serialization to complete
recipe grant before state staging and the existing guarded transport. Actual hosted
prepared model0 requestc70b18b1... has50approvals and exact whole recorded tree;
source denial starts neither native nor bridge.86fast tests PASS normal/-O.
Held snapshot acquisition remains outside export seam; ephemeral operator testbed
is not production/device authority. Factory/no-effects/upgrades/final gates remain.

Previous continuation10: see [GUARDED_EVIDENCE.md](GUARDED_EVIDENCE.md). Independently
reviewed476ae3 adds complete retained recipe + per-serialization/per-request guard,
actual guarded model1 execution with51approvals and exact full tree, initial denial
and real HEAD→POST revocation negative. 67fast normal/-O+5Rust+actual cross-language
2runsPASS. Initial private preparation/factory/no-effects/upgrades/final gates remain.

This is an implementation checkpoint, not the final evidence attachment, stage-2
suite, CI result, exact-head approval or containing-main proof.

## Exercised

- Existing fast suite: 13 tests retained. `test_hosted_export.py` added five
  synthetic controls against Gate `TrustKey`/`ArtifactStore`: missing/wrong-scope,
  revoked, altered/wrong-key grants, malformed closure descriptors, and revocation
  during serialization. The new test module first failed importing the absent
  implementation, then passed with it. No existing tests edited.
- `bridge/` consumes existing Unforged `CaosClient`; it discovers the native image
  twice on the SAME client without uploading anything. The pinned endpoint was
  `e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`.
  Both base-scoped image queries returned None (supported server-bound fallback).
- An actual existing hosted native worker executed a real recorded counter step.
  First attempt failed honestly: Ubuntu's dynamically linked driver could not
  start on NixOS (loader exit 127). A static GNU/Linux driver fixed portability;
  no deployed worker or capacity changed.
- Raw trusted-client receipt for the repeated cold probe says runner=caos,
  worker_image=`native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5`,
  executed=true, cache=none, commit=
  `e5a8a71bdab1ab8df935f66a42394204b6b3805b5d215e6d4d347fff54c77a9c`.
  Whole output tree `c839bab32d532551aadfef4f73565e5205d0171151ff843cd21a505128ce935a`
  equals the genuine stage-1 cogletd recording's first step. Ordered parent
  commits and resolver trailer were read from the raw hashed Git object and
  checked with `validate_invocation`.
- Program commit `491dda3a91368beb7554bb451df4c112e4691a8fdf64de14a0f1d4a1618a6754`,
  input commit `ff2f787b5f2e2467b762c91db5b5b7b29594859869c699a12069585317d68edd`,
  resolver `b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f`.
  Unforge SHA256 `97185610d45f5324f159465c39d2fda074fde90d72139faf818626d2f13ea93d`;
  static driver SHA256 `b645f4805fb4167c9fed139a2619b94e4259854cfa1441286816956bd2ed1f15`.

## Independent increment review and limitations

Separate session `sa-0-7d414f07` reviewed the increment. It found that a grant over
program/input alone omits the resolver and generated CAOS transport wrappers.
The actual probe sent only synthetic counter data/default public resolver, but
this is NOT a private-export qualification. `hosted_probe.py` now:

- refuses without explicit `--synthetic-transport-only`, before tools or output;
- pins the resolver independently BEFORE upload and checks it after execution;
- includes retained resolver objects in descriptors;
- never claims all generated seed/meta/worker/argument wrappers are enumerated.

The final private integration must cover the actual transport closure BEFORE
serialization/upload, not wrap a black-box subprocess with a subset grant.
The helper's unit spies prove only its callback ordering; the synthetic probe's
no-op serialize/write callbacks do NOT qualify that transport boundary.

Receipt notes are trusted-client observations over authenticated Iroh, not
independently signed server attestation. The worker image is observed, not tied
here to independent deployed source provenance. No effectful model/adapter
recording, fork, factory gate, worker canary, warm/upgrade qualification or seven
named hosted tests exists yet. The missing-config and unqualified-private CLI
controls are fast negatives, not the requested hosted tests.

## Local offline-oracle increment (not hosted qualification)

The proof driver now consumes the existing visualizer's offline fork oracle and
can emit a complete fork trace rather than its display-only projection. Explicit
`native_replay_checks.py --driver PATH` ran RED, then four native checks passed:
real nine-step Gridbot reexecution, complete fork archive independently replayed,
missing receipt refusal, and recursive duplicate/nonregular input refusal. The
existing 20 fast tests still pass normal and optimized Python. No existing tests
were removed. Gridbot has four recorded adapter actions/outcomes and **zero model
requests**; it does not satisfy model-outcome coverage.

Independent increment review `sa-0-af41fa5f` requested stricter ingress, complete
journal comparison on the materialized execution, and proper shared-module
factoring. First two are fixed and native tests rerun: the existing strict bounded
reader/parser is reused; materialization compares all journal fields except
regenerated chain identity and wall-duration observation. Shared-module factoring
is now complete in `offline.rs`; the CLI consumes the library and no longer
includes `main.rs` or suppresses dead code. The existing native CLI/refusal suite
ran all nine parameterized cases successfully. Snapshot/job ingress now also
uses the strict reader; symlink/oversize snapshot negative ran RED then GREEN.

`record_oracles.py` actually emitted complete local replay/fork archives, each
nine steps with four actions and four boundaries. Fork edits the first successful
adapter outcome; replay final tree is
`56cdcb744bea9fd625b4a961c434da1082b387b37a60abfcb803f8b239fce9d9`,
fork final tree is
`70327de13e51ba416e868b12221cd5f3c1ce2f72ac1b1e5b32bc477f3c0ec338`.
These are Runtime executions derived from the genuine existing cogletd fixture,
not new hosted jobs or model/provider runs. Initial capture was before commit;
recapture on the committed source before using its source provenance.

Read-only transport investigation `sa-0-9e134bca` identified that
CaosDriverRunner preparation/upload are interleaved; first upload is worker1,
then driver/resolver/seed/target_meta/ArgTree. CaosClient submit adds pool/engine
and bundle wrappers. An injected DriverRunner is too late for blanket zero-write
proof because resolver/seed creation already occurred. Existing native-driver jobs
return out_trace=None, unlike program jobs. Full privacy and worker-diagnostic
integration remain work; do not invent counters or silently use subset grants.

## Full synthetic hosted replay/fork increment — NOT stage2 PASS

At source `c255074d4f5006274f65c97eeaebde9993e1be45`, all nine replay steps
and all nine independently recorded fork steps actually executed on the existing
hosted native runtime. Every raw trusted-client receipt reports runner=caos,
executed=true/cache=none, with the same observed worker image and resolver pins
listed above. Every complete returned filesystem tree matches its recorded
step, including the distinct replay/fork final trees above. This is 18 fresh
receipts, not 18 names for a single result. `verify_oracle_batch.py` independently
read back each raw commit/note and checked all 18 graph/receipt/tree bindings.

Static hosted driver SHA256:
`a1c8fac758ae7130a8c7087cd0e49db2ae45696ba39885e340d9a630e1648059`.
That binary includes shared offline factoring/input-budget work, NOT the later
snapshot/job strict-ingress fix. Do not relabel its evidence as containing the
later fix. Current local driver was rebuilt and all five native checks pass.
Existing 23 fast tests pass normal and optimized Python; nine existing CLI
parameterized cases pass. Independent incremental review `sa-0-c4157cc4` approved
`c9edeab` factoring/input work only, not final stage2 or later batch changes.

`prepare_oracle_inputs.py` independently verifies native execution before
constructing each prefix input; only earlier records, current whitelisted input
fields and code/manifest blobs cross this step boundary. It imports the EXISTING
resolver from a local Unforged store after checking an independent pin, rather
than reconstructing resolver internals. Initial unseeded OS probe failed before
upload because `unforge init` alone does not create the lazy resolver branch.

Rerun preparation for each trace (`replay-trace.json`, `fork-trace.json`):

```
PATH=<private-reftable-git>/bin:$PATH python3 tests/unforged_proof/prepare_oracle_inputs.py --trace <oracle-trace> --driver <static-driver> --unforge <unforge> --resolver-store <existing-os>/dev --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f --output <new-prepared-dir>
PATH=<private-reftable-git>/bin:$PATH token-broker exec --scope caos.prod.connect --reason 'Synthetic recorded replay fork hosted batch' -- python3 tests/unforged_proof/hosted_oracle_batch.py --unforge <unforge> --bridge <bridge> --driver <static-driver> --replay <prepared-replay> --fork <prepared-fork> --output <new-batch-dir> --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f
python3 tests/unforged_proof/verify_oracle_batch.py --batch <batch-dir> --output <new-compact-evidence.zip>
```

Durable raw interim evidence is `evidence/hosted-gridbot-interim.zip` (36,781
bytes, SHA256 `b757927d9a098836aaf912215de9657f6c3051f5e46c9ac45a56b594e59cf162`).
Contains 18 raw receipt notes/returned commits/discovery/results and summary,
not credential-bearing transport config or huge object databases. Original local
oracle files plus prepared inputs remain in the task workspace.

Still NOT qualification: Gridbot has zero model requests; no full transport grant
seam, sandbox diagnostic/canary, factory entry, same-client warm/changed-pin
qualification, seven named hosted acceptance tests, final review/CI/queue/main
merge or containing-main proof. Synthetic-grant callbacks still do NOT fence all
Unforged transport objects; explicit synthetic-only refusal remains. No new fleet
capacity, paid service, GPU or deployed-runtime changes occurred.

## Reproduce capability only

Check out Unforged f9c627bcc09346b2923583037a10ac9a2fa8ea12 as a sibling of this
repository; use Git >=2.45 with reftable for jobs. Keep normal Git for HTTPS.
Do not persist/print the broker ticket or use `caos config`.

```
cargo build --locked --manifest-path tests/unforged_proof/bridge/Cargo.toml --target-dir ../unforged/target -j2
PATH=<task-private-cmake>/bin:$PATH CARGO_BUILD_JOBS=2 RUSTFLAGS='-C target-feature=+crt-static' cargo build --locked --manifest-path tests/unforged_proof/driver/Cargo.toml --target x86_64-unknown-linux-gnu --target-dir target/static-proof
# Produce the synthetic stage-1 recording with run.py per README first.
PATH=<git-with-reftable>/bin:$PATH token-broker exec --scope caos.prod.connect --reason 'Synthetic Coglet recorded-step capability probe' -- python3 tests/unforged_proof/hosted_probe.py --unforge ../unforged/target/debug/unforge --bridge ../unforged/target/debug/coglet-hosted-bridge --driver target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --recording <stage1-output> --output <new-probe-output> --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f --synthetic-transport-only
```

Pins are observations from this probe, not permanent service guarantees. An image
change must refuse until independently investigated, never normalize it away.
Raw generated evidence is in the operator-selected output directory, NOT tracked
private state or credentials. Keep the final evidence archive below 25MB.

## Genuine model request/reply increment (still NOT stage2 PASS)

Added `driver/src/model_fixture.rs`: hand-authored WAT calls the existing native
`coglet_v0.llm` import once, then returns on the reply wake. It uses existing
`async-llm-two-wakes` input/boundary fixture data (not `guest_script` or expected
journals); real NativeEngine executes both steps, then independently reexecutes
the exported archive. Provider outcome is explicitly scripted, never a live model
call. Guest does NOT inspect reply text, so this is applicability/recording proof,
not model-dependent behavior. Existing runtime's frozen hub0 build-record codec
is reused for its provenance, without changing any frozen contract.

`recorded_closure.py` requires complete committed model request/reply closure
(including initial pending and trailing boundary) before input preparation creates
any output directory or Git objects. Missing/duplicate/inapplicable/correlation
negatives are retained. A real regression control temporarily removed that check:
the native test failed because preparation reached `unforge init` on an incomplete
recording; restored check passes before nonexistent Unforged/bridge paths can run.
Both native preparation and synthetic hosted entry negative run with a genuine
one-step prefix whose model request has no reply. Four fast helper cases added;
27 fast tests pass normally and under -O; six explicit native tests pass. Initial
native command test was RED before implementation. Helpers were added before their
fast tests; do not misreport every helper case as test-first RED.

Independent separate-session incremental review `sa-0-90ec1e7f` APPROVES exact
`e999b6fbbf832a8b148adeb7f8483e1c857eac7c` for model/closure scope only. Reviewer
independently ran six native/27 fast/27 optimized tests. Later synthetic whitelist
commit and evidence are not covered by that incremental review; final exact-head
review remains required.

Actual hosted execution of BOTH model steps completed from source
`c6ff3b3ac77dcdfecbca53080942abc11d95f882`, static driver SHA256
`3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40`.
Raw receipts, commits, discovery, results and complete original trace are preserved
in `evidence/hosted-model-interim.zip` (7,009 bytes, SHA256
`06f2b4ba0fdcb651be3c30f8bb62de5c083b307ab2e07821f8612eb376dad3ae`).
Archive summary abbreviates source as c6ff3b3; full source above. Step0 receipt
`89d1991c103af1d1731393c88258b2e181ed74e63e0673f42ad9a5f1412287d6`
returned full tree `91156afc257cd101d3c6efa2d31b6ed608bc2e54d801fb1933fdcd2c25b1be38`;
step1 `9fa4390e65ec3b3d21ffb93200a97392793cb78e2d504914929db7671bbeff8e`
returned full tree `4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad`.
Both raw readbacks pass `validate_invocation`, executed=true/cache=none, same
independently pinned endpoint/image/resolver as above. This source contains strict
snapshot/job ingress fix absent from older Gridbot hosted evidence. No deployments
or capacity changed; this remains synthetic transport only, NOT private proof.

Reproduction additions (same driver/pins/broker env and private Git as above):

```
<driver> record-model . > <model-trace.json>
python3 tests/unforged_proof/prepare_oracle_inputs.py --trace <model-trace.json> --driver <driver> --unforge <unforge> --output <prepared-model> --resolver-store <stage1-unforged-os>/dev --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f
# Invoke hosted_probe.py per above twice, with --recording <prepared-model>,
# --step-index 0 / 1, separate fresh --output dirs and --synthetic-model-oracle.
python3 tests/unforged_proof/verify_model_probe.py --workspace <task-workspace> --source-head c6ff3b3ac77dcdfecbca53080942abc11d95f882 --output <new-evidence.zip>
```

Full actual pre-serialization transport grants, supported worker sandbox diagnostics
and canaries, actual factory GateService entry, same-client warm and changed
image/resolver controls, seven named qualification tests, final review/CI/native
queue/main merge and containing-main proof remain unfinished. Do not promote this
incremental synthetic model capability to any of those acceptance claims.

## Continuation 6 — retained-client hosted cold/warm control, NOT PASS

Committed source `eaa54c7ffa55dfa7689ce5fccdf97a4a3c268480` adds an explicit
synthetic-only bridge command using ONE existing CaosClient/CaosDriverRunner for
Bypass/Default/Default. No new scheduler/store/runner primitives. Existing discovery
command remains supported. Three explicit bridge CLI negatives ran RED against the
old binary, then GREEN after locked/offline rebuild. Three fast transcript
negatives ran RED missing helper then GREEN; additional duplicate-envelope control
ran RED with duplicate JSON accepted by parsing, then GREEN strict parser. Total
31 fast tests pass normal/-O; 3 explicit built-bridge checks pass. Prior6 native
runtime checks retained unchanged (not rerun this increment). Original95files/
453definition inventory remains unchanged.

ACTUAL same-client hosted control on the previously recorded synthetic model step1:

- cold commit `df8e0d3a4fbdf428318a50d84038c0eb63eb49525ae5f992bf90de5e3a9377c7`,
  executed=true/cache=none;
- default1 `36672eded7a954eed784699cc13aef67e89cb6e3a867619160d424b148411e17`,
  executed=false/cache=caos;
- default2 `53b7bbf2deddb137f96b1f279ac7b3a1400559a387df580a5cce1ebb20fe4289`,
  executed=false/cache=caos.

All complete trees match genuine model step1
`4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad`.
Image/endpoint/resolver pins unchanged. This establishes remote warm HIT controls,
not local cache hits or actual worker upgrade. Existing driver is source c6ff3b3,
SHA2563da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40.
`verify_cache_probe.py` independently re-read3 actual raw commit/note pairs from
retained trusted-client Git store. Five LOCAL revalidation negatives reject changed
expected image/resolver, missing/ambiguous receipt and default2 miss; these are
mutations/pin changes against actual captured observations, NOT hosted upgrades.
Archive `evidence/hosted-cache-interim.zip`,4888bytes, SHA256
`56949e63140d91bcc8c1f367a84d5987d77859e2b64ed74a715ae07cf7a4a7d0`.
Independent review sa-0-9329bc9f approves exact eaa54c7 cache increment only;
subsequent defensive-redaction/duplicate parsing/evidence not covered by that review.

Reproduction (private Git first; ticket only env via broker):

```
CARGO_BUILD_JOBS=2 cargo build --locked --offline --manifest-path tests/unforged_proof/bridge/Cargo.toml --target-dir ../unforged/target
python3 tests/unforged_proof/native_bridge_checks.py --bridge ../unforged/target/debug/coglet-hosted-bridge
# Under token-broker exec --scope caos.prod.connect --reason 'stage2 synthetic retained client cache control' --
python3 tests/unforged_proof/cache_controls.py --bridge ../unforged/target/debug/coglet-hosted-bridge --probe ../hosted-model-1 --output ../hosted-cache-controls-NEW --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-resolver b79d195fc03d415bb16367921ef52606e880d1263679991ee290af3f26d2fb9f --synthetic-transport-only
```

Raw controls at workspace/hosted-cache-controls. Same store reruns may legitimately
append ambiguous notes; do not delete/select receipt lines to claim PASS. Regenerate
a fresh synthetic input/probe if needed. Defensive redaction now applies to wrapper
outputs even though bridge errors are fixed strings.

`TRANSPORT_FINDINGS.md` records independent read-only investigations
sa-0-106ff8b3 and sa-0-6c5ffd9c. Public staging/wire upload splits exist but need
complete harness orchestration. Standard program job can publish `/cas/out-trace`
outside ABI root; supported deployed trace readback and real sandbox/canary evidence
remain unresolved. Native fd3 path explicitly lacks sidecar. No fake diagnostics,
new sandbox, upstream modifications or deployment. Next integrate those actual
seams before factory evaluation/signing; do not replace gaps with callback spies.
Full privacy/sandbox/factory/actual upgrades/seven qualification tests/final review/
CI/native queue merge/containing-main proof remain. NOT stage2 PASS.

## Continuation 7 — actual worker diagnostics transport resolved, NOT PASS

Resumed on hermes-worker-a2 with EMPTY workspace. Recovered pushed182e6f3, pinned
Unforgedf9c627b and all3compact committed archives (hashes independently matched).
31fast tests passed normal/-O; previous raw local object stores and builds are not
present here. Do not claim they survived host movement. New bridge built with
cargo+1.100.0 (system default1.91.1); no driver/native runtime builds yet this host.

Additive `synthetic-diagnostics` command consumes existing public CaosClient
put/submit/run/status/get APIs and STANDARD worker sidecar machinery. Actual
initial execution succeeded exact-image/fresh-miss but submitted-identity status
wasnull -> honest refusal. Inspected real CAOS source604b6024 revealed native
binding inserts `required-native-worker-image` before execution/status. Supplying
independent expected digest under this EXISTING arg lets server overwrite it and
preserve submitted=executed only when actual binding agrees. Fixed probe then
actually retrieved completed status and worker sidecar; readback is supported,
not a missing prerequisite. No upstream/deployed source or capacity changed.

Reviewed source `0c143e21fc5138c879ceb8207c9d95693291eaef` independently approved
by separate sa-0-46a7dc87 for THIS increment only. Rebuilt and actually reran that
source after review: request `7147019a668131c2aa813a8731521fbeb6d29b2b`, sidecar
`5b4c99dea1026317833f6e63b2f1dc08a4d011e3`. Separate authenticated readback,
then independent local verifier passed raw Git blob hash, request/salt/image and
status/sidecar consistency. Bridge binary SHA256
`b59f8dd689027b51a922bfa51ca76c65a2778c6f66ff533f52496196eeb0e776`.
Worker observed actual netns identifier and effective-policy env fieldsunset;
these are NOT isolation/no-live-effects certificates. No invented zero counters.

Tests35fast normal/-O,6explicit built-bridge CLI negatives PASS.3newCLI negatives
RED on old binaries thenGREEN;4sidecar guardsRED missingmodule thenGREEN.
Existing6native runtime checks retained, not rerun. Baseline95files453definitions
retained. Source and evidence reproduction in `DIAGNOSTICS_EVIDENCE.md` inside
committed `evidence/hosted-diagnostics-interim.zip`11801bytes SHA256
`8f47de7861869a69f3bc97e8e866a9e6c8486d50b234b14c991f8658161fb6df`.
Earlier failures/pre-review binaries explicitly distinguished from latest pair.
Use `verify_diagnostic_probe.py --probe ../hosted-diagnostics-reviewed --readback
../hosted-diagnostics-reviewed-readback --expected-image <existing-pin>` after
extracting archive in workspace. No old hosted Coglet batch rerun.

IMPORTANT source semantics correction: Unforged server_confirmed_nocache is
inferred from requestednocache+X-Caos-Cache:miss; inspected CAOS parser ignores
nocache query. Fresh consumed salt + actual executed receipt proves this call
executed, not universal server bypass. Full factory nonce and relevant inner
memo bypass remain mandatory. `/version=unknown` is NOT a deployed source pin.

Next integrate the supported STANDARD wrapper with existing filesystem proof
executable and genuine recorded replay/fork inputs, preserving exact ABI root and
separate diagnostics. Complete actual pre-serialization export closure + per-send
checks remains substantial work; do not copy runner/premerge/store semantics or
serialize private wire data before initial grant. Runtime/factory code remains
unedited. Genuine sandbox/effect canaries, actual resolver/image invalidation,
7named suite, final review/CI/mainqueue/containing-main proof remain unfinished.
No PR/newcards/deploy/paidservice/background jobs. NOT stage2 PASS.

## Continuation 8 — supported strict nested sandbox exercised; NOT PASS

Existing strict inner `unforge caos-sandbox-exec --mode namespace --net proxy-only`
now actually executes on the hosted worker without proxy socket/fd3 preservation,
real secrets, base/resolver mounts, deployment or primitive changes. Initial
reviewed-source4e6d749 request376607... honestly refused inner125/EROFS. Existing
`--caos-bin` and `--unforge-bin` avoid trying to rewrite the inherited read-only
/bin/caos mount; source67dc3e7 confirmed this. Follow-up independently reviewed
sourceaf9618b11b183b1c70cc157516a9936a117b1399 (sa-0-5b2ed597) then reran hosted:
requestaefbf7079dfafbba99f98dd2dbf4d39e24aef316, sidecar69b32b41f92e3c54dd70f771e969d4e7063766f8,
stdout0b4d0d045e27d1a447a0d5424c577b9e131ea11f. Actual outer/inner netns differ,
exit0, CAOS_SERVER_URL unset, fd3closed. Separate authenticated status+raw sidecar
and stdout readback passes exact raw Gitblob/invocation/image verifier. These are
observations, NOT complete no-effects proof or actual Coglet execution in this
sandbox. No canary ran; no fabricated counters.

`ISOLATION_EVIDENCE.md` records exact commands, binary/source hashes and honest
failure/success provenance. `diagnostics_probe.py --strict-isolation` and
`verify_diagnostic_probe.py --strict-isolation` rerun/verify this supported route.
41fast normal/-O +8builtBridge CLI refusals PASS; all6native replay checks PASS
on newly rebuilt static driver (same3da438d... bytes). Initial driver offline
build failed missing clap cache; locked normal fetch/build succeeded. Bridge
3f33f2c055287fc0d377cf9841d9e2d6e63b4f669215cc47ef4c19f5ac3c6b7f.
No privateGit/unforge CLI build yet on this host. Original95files453definitions
retained. Runtime/factory unchanged; no seven named hosted suite yet. Full
STANDARD filesystem proof wrapper, genuine safe canaries, complete pregrant
transport closure, factory nonce/signing, actual upgrades and final CI/merge/
containing-main remain. No PR/newcards/deploy/capacity/background jobs.

## Continuation 9 — actual recorded driver inside strict STANDARD; NOT PASS

The major next seam is now exercised, not another discovery-only probe. Existing
STANDARD worker runs unchanged static filesystem driver INSIDE strict namespace/
proxy-only, no socket/fd3/secrets forwarding. Outer publication preserves entire
ABI root with separate sidecar. Actual reviewed-source model0+1 and all9Gridbot
replay+all9fork steps match genuine recorded whole trees. Raw CAOS SHA1 objects
are independently hash-checked and recursively converted to SHA256 without
projecting empty directories. All18Gridbot transcripts reverified after capture.
Exact byte-bound worker/state/driver IDs, fresh salt/miss/exact-image observations
and authenticated completed-status/sidecar/raw-object readbacks retained.

Independent review first requested fixes (observation storage, exact bytes,
empty dirs and fresh-miss field). Fixed outer pipe before driver exec, mktemp,
complete raw tree conversion and strict confirmation in both Rust/Python.
Incrementf59b2fe68e489d7312d5a9b921a71970b5ae74f1 independently approved
sa-0-1e81ab03, rebuilt then remotely rerun. Batch conveniencesbfc4dbe/90ecb39
not final-reviewed. Bridge1dc1ba3c..., unchanged static driver3da438d... .
STANDARD_EVIDENCE.md holds full pins/commands/results/limitations. Durable raw
archiveevidence/hosted-standard-interim.tar.xz286704bytes SHA256
3ab16fe97bb88929223b8b73244608f23f4b5ccb5171e288009562cd91536cc3.

Honest failures: mistyped endpoint model1 refused preupload; corrected pinned
retry passed. Original batch tool300s timedout after9replay+fork0, no receipt for
incomplete fork1; tail explicitly ran8remaining in newdirectory and passed. No
jobs remain. Pre-review first model0 preserved distinctly, never relabeled.
51fast normal/-O +8builtBridge +6native PASS; original95files453definitions
retained. No seven named hosted qualification suite/factory changes yet.

Full initial private closure grant/per-send authorization, genuine safe canaries,
actual factory nonce/evaluation/signing, changed resolver/image, final review/
requiredCI/nativequeue/main/containing-main remain. No stage2PASS, no PR/newcards/
deploy/capacity/paidservices. Next complete authorization using the now simple
public STANDARD staging/wire seam, NOT copied runner/premerge semantics; factory
must retain fixture-only authority and refusal/no consumed challenge on infra
failure. No diagnostic-only rediscovery necessary.
