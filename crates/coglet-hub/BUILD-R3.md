# Trusted local R3 publication

The default builder and profiles without `r3` retain the integer-only path.
`build::Profile::r3_wat` adds protected provisioning to the existing saga Recipe;
it is not a request parameter or a new operator/wire operation. `Builder::new`
loads that provisioning. `Builder::new_r3` accepts an already verified local
`R3Profile` for embedding and hermetic integration tests. No installed-update
behavior is added: the deploy saga still sets `old_tip = None`.

## Provisioning

Provision a `build::R3Config` only in service-owned protected Recipe configuration:

- `trust`: the exact `coglet_transform::TrustRoot`, with reviewed GitHub head,
  required jobs, native x86_64/aarch64 archive identities, executable/compiler/
  lock/corpus/results digests, and the selected **native host** artifact;
- `run_id` and `run_attempt`: the exact successful run (not a branch label);
- `issuer_secret_file`: absolute, nonsymlink, service-owned regular file, mode
  0600, containing exactly 32 raw bytes for the local admission issuer;
- `issuer`: independently provisioned issuer public key, lowercase hex;
- `engine_gate_sha256`: the approved `coglet-transform-gate` executable SHA-256
  in that same authenticated native archive.

The verifier fetches exact evidence with the protected GitHub client. No caller
JSON can construct `CiQualification`. Missing/foreign/stale evidence, unsupported
host, changed engine, mismatched issuer and missing executable refuse the opt-in.
Test fixtures exist only in dev-dependencies and call the same verifier with
explicit synthetic CI envelopes; they are not real native CI receipts.

## Bounded profile

This builder admits only independently pinned finite WAT/source, manifest and
**rewritten output** identities. It uses the authenticated production transform
executable, checks its exported source against retained CI source and the exact
profile, then gates the output. The authenticated native gate executable contains
the same pinned WAMR interpreter build/configuration executed in native CI.
It runs two fresh step-zero instances with fixed empty-event inputs and seeds
0 and `0xfff8000000000042`. Budgets: 10,000 compute units, at most two memory
pages, 64 KiB host bytes/stack, 16 MiB runtime pool, one-second step deadline.
The manifest must specify `wamr-2.4.3-fast-cumulative-v1`, at most two memory
pages, and at least 10,000 compute units. Only the frozen `coglet_v0.log` import
has an inert bounded implementation; other host effects refuse. The native
supervisor separately caps process CPU, address space, output and wall time.
This is an explicit thin publication gate, not a general journal replay,
AOT qualification, arbitrary host-import adapter or general R5 claim.
It is an execution smoke test at the fixed gate budgets, not proof of behavior
at a manifest's larger compute allowance. The exact-output pin identifies bytes,
not exhaustive input or budget coverage. The native gate is Linux-only;
other platforms compile an explicit refusal, never a substitute engine.

Order is rewrite → structural/ABI + exact-output/native execution gates → signed
archive preflight under a disposable non-router key → IMMEDIATE Store transaction
with exact current delegation/expiry and scoped admission → router signature →
atomic artifact, digest-addressed evidence and audit retention. The token binds
authoritative router/publisher, target/runtime, source/original/output/profile,
CI/evidence, delegation generation/digest, and the exact old/new tip/generation.
The same operation with changed qualification/issuer conflicts; identical
retrieval after expiry grants no new signing or dispatch authority. The remote
runtime's original ref CAS remains the real race fence.

The closed `hub-build-record` is unchanged. Original/output, production tool,
source closure, native gate, compiler/lock, corpus/results, raw GitHub/archive
bytes and local gate report live in the existing Store as SHA-256-addressed
blobs. `build/evidence.json` inside the signed archive indexes those bytes. The
bounded reproduction JSON carries that index and scoped admission, never blobs.
No second store or custody transaction is introduced.

## Tests

Build production tools before running the integration binary (required Rust CI
already runs `cargo build --workspace --locked` before workspace tests):

```sh
cargo build --locked -p coglet-transform --bins
cargo test --locked -p coglet-hub --test build
cargo test --locked -p coglet-transform --test admission_regressions
python3 crates/coglet-transform/check.py --native-leg
```

`rewrite_before_build_commit` and
`authenticated_transform_publication_admission` exercise the real tool/native
gate and signed archive path with a synthetic **dev-only** CI envelope, retaining
and re-reading the exact original/output/index through Store. Refusal, delegation
race, rollback and integer regressions are in the same required workspace suite.
Native CI independently executes and retains the production gate, both native
architectures and the unchanged 960-case WAMR/Wasmtime/oracle comparison. Final
H-transform/runtime qualification remains the final integration owner's gate;
this split alone does not mark it qualified.
