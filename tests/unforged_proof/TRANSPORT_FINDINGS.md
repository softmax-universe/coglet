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
