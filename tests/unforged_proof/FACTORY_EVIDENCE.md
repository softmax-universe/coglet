# Factory hosted integration increment — NOT stage2 PASS

## Scope and independent reviews

Implementation `8e5b0d2ac4155ab0d25b4d5a0e762a52f7ee96d5` was independently
APPROVED by `sa-0-f8b94107`; real native three-step archive reexecution was
independently checked as well. Probe/credential isolation followup
`a6eec019a1c55fea2ffb7bd8c81727ba7eac9c7b` was APPROVED by
`sa-0-74e65976`. Exact signed-artifact retention/offline verifier
`9fcd765544f2956d3a7aa637c37eeff2c1ed630c` was APPROVED by
`sa-0-e5cec5ac`. These are incremental reviews, not final stage2 approval.

`GateService.evaluate_hosted` now actually precedes the existing signing call.
It keeps the native suite's behavioral verdict, acquires its complete genuine
trace using existing `RuntimeService.export_trace`, and requires every ordered
hosted step/tree/invocation/transcript identity before publication. No caller
report can replace `passed` or the suite/candidate binding. Native tool identity
is distinct from hosted identity, preventing native downgrade of a hosted
challenge. Hosted identity includes all harness Python/Rust sources, exact
bridge/driver/worker bytes and operator endpoint/image/authority configuration.
The plugin is inside the same-UID trusted operator boundary, like native tools;
its summary is NOT independent server attestation. Strong transcript verification
and authenticated CAOS transport are still required by the actual plugin.

The complete challenge, index and a fresh operator per-evaluation nonce derive
each salt consumed by the actual STANDARD request. The extra nonce matters:
retrying a challenge after infrastructure failure must not repeat an already
cached request. Only existing `accept_fixture` consumes the challenge. Unit
failure/retry tests independently inspect the challenge SQLite row remaining
NULL and no evaluation artifact. Fixture-only and production refusal remain.

`FactoryTransport` reuses both independent operator source and recipe grants,
existing guarded preparation, existing public STANDARD APIs and strict inner
namespace/proxy-only execution. It introduces no scheduler, router store or
Unforged primitive. STANDARD has no resolver input: this does not replace the
separate native-path resolver invalidation qualification. Native acquisition is
local and explicitly outside the later export seam; its subprocesses receive no
CAOS ticket/URL. The CLI is an ephemeral OPERATOR synthetic testbed, not private
device or production authority.

## Actually exercised tests and failures

- Original95files/453source definitions retained.86prior fast cases retained;
  7new factory service cases initially RED missing module then GREEN.3new
  transport negatives were NOT test-first.96fast cases pass under the locked
  sim environment, normally and under `-O` (fake native seam only under `-O`;
  the real native evaluator deliberately refuses optimized Python).
- The new service tests require sim's existing cbor2 dependency: use
  `uv run --project sim --locked python -m unittest discover -s tests/unforged_proof -q`.
  Plain system Python's missing cbor2 was a real initial environment failure,
  not a gate pass. No new dependency or lock change was needed.
- Retained sim gate/evaluation tests:24PASS/5native-env SKIP in the unconfigured
  run. Skips are NOT native qualification. Actual configured native regression
  results are retained separately in the evidence archive.
- Native archive exporter code was not test-first. First actual compatibility
  probe failed honestly: opening a second RuntimeService while the first held
  the WAL correctly refused its own file lock. Reusing the existing open owner
  fixed it. `factory_native_checks.py` then executed a real native3step suite,
  imported/reexecuted its complete archive with the existing driver and checked
  all prefix/prior state trees. No handcrafted trace converter or second executor.

## Actual hosted behavioral-rejection control

Source `a6eec019a1c55fea2ffb7bd8c81727ba7eac9c7b`, default max-steps1.
Actual `challenge_fixture -> evaluate_hosted -> accept_fixture` executed all3
native-recorded steps hosted. Whole ABI trees match their exact native oracle.
`report.passed=false` is retained: Gridbot did not reach its goal in one action;
this is an honest policy rejection, not stage2 or behavioral PASS.

Requests:
- `39ce03a937d6311fda3bda232ff9254998f23e22`
- `08bf27c55504831ad80b478c316a434c67931745`
- `9e0d8200a860d38b14b331c7979057b9b25110f9`

Actual fixture artifact ref
`2ade25765adb2837f939137df9ab95825c3d2b07eb2be78d12deaff76fb22b85`;
one-use second acceptance refused and production verification refused. This
first probe retained decoded evaluation but NOT original signed envelope before
key deletion; the later reviewed probe fixes retention. Do not claim the raw
artifact is present for this rejection run.

Independent offline verifier rechecked3actual completed statuses, raw sidecars,
request/challenge/attempt salts and full raw output object graphs.9mutations of
those real transcripts (missing executed row, duplicate executed row, changed
expected image) refused LOCALLY. They are not remote upgrades/executions.

## Actual hosted behavioral PASS control (still NOT stage2 PASS)

Reviewed source `9fcd765544f2956d3a7aa637c37eeff2c1ed630c` with max-steps16:
actual native Gridbot reached its goal, all9native-recorded steps then executed
hosted with unique fresh requests/exact image/full-tree equality. The actual
service signed fixture evaluation
`c539938b1450f378d96cc51b3e79813dea4132457a84f02f5d325af3c1a85fe8`;
existing admission preserved behavioral passed=true, second acceptance refused,
production verification refused. Raw signed envelope IS retained for this run.
Final request `b642a51573a0511233a1d5af232939a0d053e935`, complete final ABI tree
`81943826af8991c778926dfd2b93a3975c4f82c8185e46a493d6cd6ccc89d090`.
Every earlier step/request/tree/sidecar is retained verbatim in raw transcripts
and independent-verification.json; no output projection. Independent verifier
with `--require-artifact` PASS9,27LOCAL missing/ambiguous/image receipt mutations
refused. Programmatic aggregate across the two distinct controls:9unique PASS
requests plus3unique rejection requests. These are12new actual factory steps,
not reruns of prior replay/fork/model batches.

Configured retained native regression attempt timed out at its wrapper's180s
allowance. No complete parameterized result is claimed. Its orphan pytest process
and refused cleanup are documented in factory-native-regressions.txt. The process
was confirmed gone on the final readback; no regression process is left running.
This is pending regression work, not a flake waiver. The separate actual native archive
and both complete hosted factory controls succeeded as described above.

## Reproduction and pins

Unforged remains `f9c627bcc09346b2923583037a10ac9a2fa8ea12`.
Bridge SHA256 `d901e7f1066cabcb12f75f5a5879f7e4f2dac1f7f4165a3e2cbe8a9526788733`;
static driver `3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40`.
Native daemon/proof were rebuilt locked with toolchain1.100.0/static GNU target;
exact binary digests and source heads are in each run's command.json.

```
RUSTFLAGS='-C target-feature=+crt-static' cargo +1.100.0 build --locked --manifest-path Cargo.toml -p coglet-runtime --features native-engine --bin cogletd --target x86_64-unknown-linux-gnu --target-dir target/static-proof -j2
RUSTFLAGS='-C target-feature=+crt-static' cargo +1.100.0 build --locked --manifest-path sim/native/Cargo.toml --target x86_64-unknown-linux-gnu --target-dir target/static-proof -j2
# From workspace beside repo/ and unforged/, ticket environment only:
token-broker exec --scope caos.prod.connect --reason 'Reviewed synthetic hosted factory proof' -- uv run --project repo/sim --locked python repo/tests/unforged_proof/factory_probe.py --synthetic-transport-only --max-steps 16 --daemon repo/target/static-proof/x86_64-unknown-linux-gnu/debug/cogletd --proof repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-sandbox-proof --driver repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --bridge unforged/target/debug/coglet-hosted-bridge --output NEW-FACTORY-PASS --expected-driver-sha256 3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40 --expected-bridge-sha256 d901e7f1066cabcb12f75f5a5879f7e4f2dac1f7f4165a3e2cbe8a9526788733 --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5
uv run --project repo/sim --locked python repo/tests/unforged_proof/verify_factory_probe.py --batch NEW-FACTORY-PASS --require-artifact --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5
```

## Durable interim archive

`evidence/hosted-factory-interim.tar.xz`,277268bytes, SHA256
`1e49707f200f7ac91f4bd7176206951ae755ff19311ec95d3c941ee93b23909f`.
Contains both complete actual factory controls, their exact native oracles,
all raw guarded transcripts/status/output objects, request/step/scope/tool pins,
PASS raw signed artifact, independent verifications, native compatibility and
regression timeout diagnostics. No ephemeral operator keys/authority directories
or CAOS tickets. Solid compression retains exact bytes. Prior replay/fork/model
archives remain separate and unchanged.

## Remaining qualification

Safe genuine enforcement canaries/no-live-effects evidence, actual retained-client
resolver/image invalidation, seven named hosted suite, full final independent
exact-head review, required CI/native queue/main merge and containing-main proof
remain. Requestednocache+observedmiss still is not universal bypass attestation.
No stage3, device Git, effect rewrite, GPU, new capacity, paid service or deployment.
NOT stage2PASS. No prior completed replay/fork/model batches were rerun.
