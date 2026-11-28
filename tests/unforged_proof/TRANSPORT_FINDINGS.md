# Remaining transport and diagnostics integration — source findings, not PASS

Inspected Unforged f9c627bcc09346b2923583037a10ac9a2fa8ea12. Independent read-only
investigations sa-0-106ff8b3 and sa-0-6c5ffd9c; these are source findings, not
hosted qualification. Preserve explicit synthetic-only refusals.

## Pre-serialization grants

`execute_program_as_merge_with_env` creates/resolves resolver and premerge before
final runner entry (`src/driver.rs:2223–2227,2283–2324`), then creates seed commits
(`2373–2455`). Runner-only grant verification is too late for an initial denial
that promises zero private writes. `CaosDriverRunner` interleaves upload of worker,
driver, resolver, seed, metadata, arguments, operands and ArgTree
(`src/caos.rs:3068–3195`); client submission adds engine/pool/bundle objects
(`1473–1547`). There is no complete public prepare-only high-level runner.

Existing public split candidates, NOT integrated:

- `MemoryObjectBatch::stage_blob/stage_tree/stage_commit` serialize in RAM;
  `flush_to` writes separately (`crates/unforged-gitobj/src/store.rs:63–116,186–190`).
- `serialize_caos_object`/`encode_caos_tree` plus
  `CaosClient::put_wire_object` separate wire preparation from upload
  (`src/caos.rs:202–209,261–276,943–980`). Recheck authority before sends.
- `put_wire_object` itself does HEAD then POST. Public `request_with_headers`
  is an existing lower transport seam (`1824–1859`), not a new transport/store.

A harness orchestrator must validate the actual complete retained graph, authorize
initially before staging private bytes or writes, then recheck at each crossing.
Do not grant a subset then delegate to the opaque runner. Wrapper private metadata
also needs an initial retained grant; avoid circular pregrant wrapper generation.
A synchronous recheck is not atomic distributed revocation. Do not remove the
synthetic-only restriction until the actual seam is exercised independently.

## Standard-job sidecar candidate

Correction to loose earlier shorthand: the proof executable itself is an ordinary
filesystem program. It reads/removes `proof-input.json` and writes the complete
ABI filesystem (`driver/src/main.rs:72–102`). Unforged supplies its surrounding
merge/fd3 machinery. Native-driver routing sets `out_trace=None`
(`src/caos_runner.rs:2054–2077`), so writing another fd3 blob alone does not provide
a returned diagnostics sidecar.

The existing STANDARD program-merge route imports `worker1` and argument trees
(`4797–4893`), executes a wrapper through supported sandbox (`2709–2743`), then
extracts internal `{.caos-kind,.caos-out-trace?,out}` (`3490–3593,5109–5191`). Only
`out` becomes the returned root; the sidecar need not contaminate ABI equality.
A future opt-in wrapper can execute the same proof executable in a fresh complete
state directory, then use existing `caos put STATE /cas/out` and
`caos put DIAGNOSTICS /cas/out-trace`. Merely writing JSON to out-trace does NOT
register its OID; use the existing put/ingest metadata (`3070–3103,3553–3575`).
Include a consumed request salt to bypass runner-local memo (`4982–4989`) as well
as authenticated outer no-cache. This is a design candidate, NOT an executed job.

Remaining gaps to resolve before implementation can qualify:

- Runner sets effective sandbox/network env (`5027–5062`), but sandbox explicit
  `env_clear` pass lists omit these fields (`caos_sandbox.rs:907–970,1799–1847`).
  Reading a configured env value would not prove effective isolation anyway.
- Auto may choose Worktree; that fallback does not enforce the network policy
  (`caos_sandbox.rs:907–990`). Never claim proxy-only proves no network.
- Proxy denies some routes with observable403, but allows CAOS submit/evaluate/
  child routes (`caos_runner.rs:1006–1025,1192–1199`). It is not a blanket live
  effects fence. Need genuine observations and canaries, not invented zero counts.
- Runner POST includes nonce/request/out_trace (`578–607`), but
  `CaosClient::run_arg_tree_detailed` exposes only result/cache/image
  (`caos.rs:1594–1651`). Verify a supported deployed trace-readback route before
  relying on this sidecar. Integration test `caos_unforged_runner.rs:2804–2810`
  establishes server receipt of a field, not client retrieval or hosted proof.

No Unforged source, deployed worker, capacity or frozen contract changed. Full
privacy/sandbox/factory/upgrade/seven-test/CI/queue/containing-main gates remain.

## Continuation 7: actual supported readback resolved; do not file an absence prerequisite

CAOS source `604b6024166e5eb4ffa41e3555c2f5f0bd9d06b6` (read-only checkout)
provides `/status/<ArgTree>?all=1` with `out_trace` (`server/src/status.rs:398–436`).
Its native request binding inserts `required-native-worker-image` before dispatch
(`compute.rs:678–724`); status tracks that **bound** identity. Supplying the
independently expected digest under that existing arg allows submitted=executed
only when the server's own overwrite agrees. It cannot forge worker identity.
A completed plain status may return null by design; `all=1` retains completed work.

Actual reviewed-source probe `0c143e21fc5138c879ceb8207c9d95693291eaef` executed
STANDARD worker request `7147019a668131c2aa813a8731521fbeb6d29b2b`, fresh salt,
actual cache-miss/exact-image receipt. Completed status names sidecar
`5b4c99dea1026317833f6e63b2f1dc08a4d011e3`; a SEPARATE authenticated readback
retrieved its exact blob bytes. `verify_diagnostic_probe.py` independently checks
Git SHA1/raw bytes, status identity, salt, receipt disposition and observations.
This closes supported sidecar transport, not the full Coglet wrapper or isolation.

The worker actually observed `net:[4026532663]` and both effective-policy env
fields **unset**. No fake model/adapter/network zero counters were added. No live
effects/canaries were attempted. `/version` returns `unknown`; inspected CAOS HEAD
is not therefore a deployed-source attestation. Earlier unbound request3208ca67
returned null status; preserve that honest failure, not an absence conclusion.

Also inspect bypass semantics, not a field name: pinned Unforged sets
`server_confirmed_nocache` when a requested nocache run has X-Caos-Cache=miss
(`caos.rs:1638–1641`). The inspected CAOS run parser uses req, not nocache
(`compute.rs:556–586,639–652`). A new consumed salt plus genuine miss establishes
actual execution for THIS invocation; do not claim a server-wide bypass API.
Existing factory nonce/salt and inner runner memo controls remain mandatory.

For actual full replay/fork/factory integration, reuse this supported STANDARD
wrapper plus the existing filesystem proof executable and exact ABI root output;
do not add diagnostics to that root. Initial complete private grant before staging
and per-crossing revalidation remain unimplemented. Do not serialize private wire
objects before the initial grant merely to discover their descriptors. No copying
seed/premerge/runner/store semantics into a second implementation is authorized.
