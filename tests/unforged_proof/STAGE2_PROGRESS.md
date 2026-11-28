# Stage 2 increment: hosted capability discovered, NOT qualification PASS

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
regenerated chain identity and wall-duration observation. Proper factoring remains
OPEN: the new library currently compiles main.rs as an internal dead-code module.
Do not call this final implementation approval. Add further journal-mismatch and
oversize/symlink controls, run existing CLI tests, and refactor before final review.

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
