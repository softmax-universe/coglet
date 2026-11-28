# Actual strict nested sandbox increment — NOT stage2 PASS

## Source and review

Coglet branch `stage2/t_01a21c4b5645783f`, executable source
`af9618b11b183b1c70cc157516a9936a117b1399`; independently approved incremental
review `sa-0-5b2ed597` (not final stage2 approval). Earlier increment `4e6d749`
was approved separately by `sa-0-310564c2`. Consume unchanged Unforged
`f9c627bcc09346b2923583037a10ac9a2fa8ea12`. No primitive, runtime deployment,
fleet capacity, protocol or factory authority changed.

Bridge SHA256 `3f33f2c055287fc0d377cf9841d9e2d6e63b4f669215cc47ef4c19f5ac3c6b7f`.
Static filesystem proof driver rebuilt on this host:
`3da438d073ef5c642e20f0522ed28a38fca492b37fc8a0a541d5d6b6295cbe40`
(same bytes as prior model increment, NOT executed inside this sandbox probe).

Endpoint operator pin:
`e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`.
Actual receipt worker image:
`native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5`.
No resolver graph was used/qualified by this direct STANDARD probe. The existing
native path's resolver pin remains separate.

## Actual executions and failures

1. `hosted-isolation-reviewed`: source `4e6d749`, actual request
   `376607475ebf50fdeacf6da08231ef1d2d99aa5a`, cold outer execution, inner
   strict sandbox **exit125 / Read-only file system**; sidecar
   `e75168fa95a174915c44c576725731f67a08c8bf`. Refused, no fallback.
2. `hosted-isolation-binmount`: source `67dc3e7`, actual request
   `c78f3edbe89531e129be58e1e71439a46ed5123b`, inner exit0. This run predates
   the follow-up approval and renamed schema. Do not treat it as final-source proof.
3. `hosted-isolation-final`: independently reviewed source `af9618b`, actual
   request `aefbf7079dfafbba99f98dd2dbf4d39e24aef316`, cache_hit=false and
   expected worker image. Sidecar `69b32b41f92e3c54dd70f771e969d4e7063766f8`;
   raw stdout blob `0b4d0d045e27d1a447a0d5424c577b9e131ea11f`.
   Observed outer `net:[4026532663]`, inner `net:[4026532724]`, exit0,
   CAOS_SERVER_URL unset and fd3 closed.
4. `hosted-isolation-final-readback`: separate authenticated GET-only status,
   raw sidecar and raw stdout. Offline strict verifier checks exact raw Git blob
   hashes, request/salt/status/executed-image composition, distinct namespace,
   field schema and exact stdout. PASS for these observations only.

The deployed supported sandbox accepted explicit `namespace` / `proxy-only`
without a proxy socket, fd3 preservation, resolver/base or real secret mounts.
The first EROFS was not a missing deployment or namespace capability: pinned
sandbox copies existing `/bin` mounts then attempts to rewrite `/bin/caos` when
no explicit binary is supplied. Reusing its EXISTING `--caos-bin` and
`--unforge-bin` options succeeded, with no source/deployment fix.

## Commands actually run

From task workspace, using credentials only in the broker-launched environment:

```sh
cargo +1.100.0 build --locked --offline --manifest-path repo/tests/unforged_proof/bridge/Cargo.toml --target-dir unforged/target -j2
token-broker exec --scope caos.prod.connect --reason 'Reviewed source strict nested namespace observation and raw sidecar' -- python3 repo/tests/unforged_proof/diagnostics_probe.py --synthetic-transport-only --strict-isolation --bridge unforged/target/debug/coglet-hosted-bridge --output hosted-isolation-final --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5
token-broker exec --scope caos.prod.connect --reason 'Independent read only raw status and strict sandbox output followup' -- python3 repo/tests/unforged_proof/diagnostics_probe.py --synthetic-transport-only --bridge unforged/target/debug/coglet-hosted-bridge --output hosted-isolation-final-readback --expected-endpoint e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce --request aefbf7079dfafbba99f98dd2dbf4d39e24aef316 --result-oid 0b4d0d045e27d1a447a0d5424c577b9e131ea11f
python3 repo/tests/unforged_proof/verify_diagnostic_probe.py --strict-isolation --probe hosted-isolation-final --readback hosted-isolation-final-readback --expected-image native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5
```

Use new output directories and fresh salts when rerunning. Each command.json
retains exact argv and bridge hash. The returned `server_confirmed_nocache` is
still inferred requested-nocache+miss, not a universal server bypass guarantee.
Fresh salt/request and actual outer miss establish this invocation executed.
Identical stdout blobs across invocations are expected and not execution receipts.

## Tests and scope limits

41 fast unittest cases PASS normally and under `-O`; 8 built-bridge CLI
refusal cases PASS. Two new CLI controls ran RED against old binary, then GREEN.
Four observation cases first ran RED missing API, then GREEN. Bash syntax and
stub125 failure controls were added after implementation (not claimed test-first).
All 6 explicit native replay checks PASS using rebuilt static proof driver;
41fast/8bridge are not hosted qualification tests. Original95file/453definition
baseline inventory retained unchanged.

This establishes usable strict inner sandbox observations, NOT complete replay
no-live-effects or private export qualification. No model/adapter/network canary
ran. The full recorded filesystem driver has NOT run inside this STANDARD inner
sandbox, and its exact ABI tree has NOT been compared on that route. Initial
complete-closure grant before serialization and per-write/send checks, actual
factory evaluation/signing, stale image/resolver controls, seven named hosted
qualification tests, final exact-head review/required CI/queue merge and
containing-main proof remain. Keep synthetic-only/private fail-closed restrictions.

Next integrate the existing static filesystem proof executable into this now
exercised strict STANDARD wrapper, with genuine recorded step inputs, diagnostics
outside `/cas/out`, actual safe enforcement canaries, complete transport grants
and factory nonce binding. Do not add another sandbox, scheduler, router store or
copy Unforged merge semantics. `ISOLATION_EVIDENCE.md` is an intermediate receipt,
not stage2 acceptance.
