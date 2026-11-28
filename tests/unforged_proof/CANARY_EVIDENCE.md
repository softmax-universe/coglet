# Hosted loopback enforcement increment — NOT stage2 PASS

Exact executable source `55e4463ed6ec5e6aabe2d58ced4bfbaf9a8e168b` was
independently APPROVED by separate session `sa-0-553c5401`, after first review
`sa-0-ea4a6698` found a genuine backlog/shutdown counter hole. Native regression
queued five real TCP connections while the owned listener was stopped: old binary
reported2/1 rather than6/1 (RED); drain-to-WouldBlock, observing stop before the
final pass, fixed it (GREEN). Drain budget128 and lifetime120s fail closed.
The listener joins before final counter verification; the worker EXIT trap kills
and joins it on early failure. No persistent listener/service is installed.

## What actually ran

Existing strict STANDARD namespace/proxy-only execution, no proxy socket/fd3/
secret mount. Outer trusted worker starts two real **synthetic loopback-only**
listeners with ephemeral ports (roles model and adapter), then performs nonce
handshakes to prove both listeners are reachable outside the namespace. Each
listener records one contact, with zero bad handshakes. Inner probe attempts
connections to exactly those two 127.0.0.1 ports; it cannot accept DNS or a
non-loopback destination. Both actual inner attempts returned `refused`.
Inner self-loopback positive control worked (`loopback-up`). Normal recorded
Runtime.step then ran; final joined listener counts stayed1/1 with bad0.

A fourth bounded fixed-token line is consumed through the existing pipe into
outer-only files before driver stdout, never a guest-writable observation file.
Actual post counters are retained even on failure; malformed/connected/extra
contact/listener failure refuses output publication. Diagnostics are separate
`/cas/out-trace`, not entries added to the ABI tree. Both source and recipe grants
remain supplied by the operator; no keys enter bridge/guest. CLI keys remain an
explicit ephemeral synthetic OPERATOR testbed, not production/device authority.

Three new actual reviewed-source executions (not old batch reruns):

| Oracle/step | Authenticated request | Exact whole recorded ABI tree |
|---|---|---|
| model0, genuine Wasm model request | `1378f6e066174684a0c8e282697af2f5426040f4` | `91156afc257cd101d3c6efa2d31b6ed608bc2e54d801fb1933fdcd2c25b1be38` |
| model1, recorded reply applicability | `5e7508a1fcfcd2d20bbe189cde66c135090eb58b` | `4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad` |
| Gridbot2, recorded adapter continuation | `fc69b3df359f1d5be705d9ec0177c0e20aef269b` | `e498a3d13774b9376b9f967b3e6d56eb44e412541a6933fbc0884195592f7508` |

All three: fresh salt, observed cache miss/exact pinned worker image, actual
completed status plus raw sidecar/full object graph independently rechecked by
`verify_loopback_probe.py`. It requires the canary, exact independent source,
driver/bridge/image pins and original trace bytes. Legacy isolation verification
still permits absent canary data and does NOT upgrade historical evidence.
Programmatic aggregate:3executions/3unique requests. Raw sidecars:
`65ed870c3ff7bda371b06d0835a5a6f310c0ded3`,
`a417a74d909cf15bd3543ec902c283bce74b1a70`,
`e0110d739d48109b1df25522e51630bab5fe6218`.
All report `1-1-0:refused,refused,loopback-up:1-1-0`.

## Pins and reproduction

Unforged `f9c627bcc09346b2923583037a10ac9a2fa8ea12` unchanged.
Driver **changed** to include canary subcommands, SHA256
`2528aa44145a5f84b466db51b4a3b51e3f5e5a8ecbc8b66b4496a471d75fe4d4`.
Bridge SHA256 `6b6ce8fe3f3c0ec5228d9bbb4cf69da8892f07aac93fd46042746e37edaba753`.
Worker `native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5`.
Endpoint `e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`.
No worker deployment/capacity/identity was changed. This is NOT actual worker-image
invalidation. Requested nocache+observed miss is not universal bypass attestation.

Build commands remain the locked/static driver and locked bridge commands from
STANDARD_EVIDENCE.md, toolchain1.100.0. Archive acquisition helper executes genuine
local model recording and existing Gridbot replay, then existing prefix/snapshot
APIs BEFORE selecting export. From workspace beside repo/ and unforged/:

```
python3 acquire_canary_oracles.py
# Ticket environment only, no persistent URL/config:
token-broker exec --scope caos.prod.connect --reason 'Reviewed synthetic loopback proof' -- uv run --project repo/sim --locked python repo/tests/unforged_proof/prepared_probe.py --synthetic-transport-only --trace canary-oracles/model.json --snapshot canary-oracles/model-0-snapshot.json --driver repo/target/static-proof/x86_64-unknown-linux-gnu/debug/coglet-unforged-proof --bridge unforged/target/debug/coglet-hosted-bridge --output NEW-MODEL-0 --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-bridge-sha256 6b6ce8fe3f3c0ec5228d9bbb4cf69da8892f07aac93fd46042746e37edaba753 --coglet coglet:1111111111111111111111111111111111111111111111111111111111111111 --step 0 --oracle model
uv run --project repo/sim --locked python repo/tests/unforged_proof/verify_loopback_probe.py --job NEW-MODEL-0 --trace canary-oracles/model.json --step 0 --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5 --expected-source 55e4463ed6ec5e6aabe2d58ced4bfbaf9a8e168b --expected-driver-sha256 2528aa44145a5f84b466db51b4a3b51e3f5e5a8ecbc8b66b4496a471d75fe4d4 --expected-bridge-sha256 6b6ce8fe3f3c0ec5228d9bbb4cf69da8892f07aac93fd46042746e37edaba753
```

Use model1-snapshot/step1 for reply control, gridbot.json/gridbot2-snapshot/step2
and the `coglet:bbbb...`64b target for adapter control. Exact original CLI argv,
source acquisition commands and hashes are retained in each command.json.

## Tests and limits

100fast normal/-O PASS (96retained+4canary verifier cases); native canary3PASS,
6existing native replayPASS, Rust bridge5PASS/1explicit cross-language ignored
(not counted PASS). The initial native readiness test genuinely failed before
implementation; four fast cases RED missing verifier then GREEN. Existing shell
refusal assertion retained with additive stub listener setup. Reviewer additionally
exercised shell malformed/probe/pipeline/extra-contact/listener failures and a
successful stub, not remote execution claims. Existing95files/453definition
baseline retained; modified shell fixture preserves the existing refusal test.
Configured retained-native/gate regressions:31distinct selected cases PASS,
including real Gridbot4actions/9trees and Cartpole16actions/33trees, native gate
pass/reject for both worlds, evaluation and adversarial/service controls. The
first direct pytest command hit the tool's420s timeout after18passed; the exact
remaining13cases completed exit0 in88.71s (no rerun of completed cases).
`aggregate_native_regressions.py` compares actual collected IDs with both logs/XML:
31expected/31unique passed/no missing. This is an aggregate of two runs, NOT a
single uninterrupted31case suite pass. Earlier180s timeout remains honest history.
No regression process remains. Native binaries rebuilt locked/static on this
host. Broader original source inventory is not claimed fully executed here.

This is genuine enforcement for two synthetic local endpoints, NOT a universal
no-live-effects certificate or a provider-dispatch audit. Normal driver code uses
recorded Runtime.step host calls without RuntimeService/provider/adapter clients;
the endpoint controls alone do not count all attempted syscalls or live APIs.
Final named qualification must compose these observations with executable-source
closure/recorded outcomes. Actual resolver/image invalidation, complete cache
freshness scope, seven named hosted tests, final independent exact-head review,
required CI/native queue/main merge and containing-main proof remain.

Durable `evidence/hosted-canary-interim.tar.xz` retains all3raw guarded transcripts,
scopes, original traces/snapshots, commands, sidecars, full object graphs and
independent verifications plus acquisition/packaging helpers. No ticket/key/store.
This is interim evidence only. NOT stage2 PASS.
