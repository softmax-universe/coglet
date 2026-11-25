# Trusted-local router publication builder

`coglet_hub::build` implements the ADR-0018 **publication** seam. It adds no wire
method, service listener, actor authority, lifecycle receipt, promotion bypass or
coglet-key access. The operator service/saga must supply its own authenticated
UID, admitted operation ID, current grants and receiving-runtime boundaries.

## Deliberately narrow qualification

The first supported profile is `integer-WAT-v1`, a **finite, operator-reviewed
artifact allowlist**. Protected local provisioning supplies the target repository,
coglet/runtime identity, profile ID and exact SHA-256 pins for `guest.wat`,
`manifest.json` and the approved compiled module. Pins must be obtained by
independent review, not copied from a guest or accepted from the operator request.
There is no default profile, caller qualification boolean, shell recipe, remote
repository URL or arbitrary compiler invocation.

The builder reads the two exact Git blobs at the requested full source commit,
checks their pins before parsing, compiles real WAT with locked `wat`, validates a
FLOATS/SIMD/threads-disabled feature set and the existing engine-independent ABI,
then checks the independently provisioned exact-output gate pin. The publication
transform is the identity transform: original and rewritten digests must agree.
This is **not ADR-0003/R3 qualification**, a general Rust/WASI compiler, a native
runtime execution proof, journal replay or an M3 promotion gate authority. Float,
SIMD, ambient-WASI and unapproved source/output are denied. Selecting another
profile requires separate qualification; it cannot silently fall back to this one.
Existing promotion policy and its gate requirements still apply at dispatch.

## Custody and signing

- `Builder::compile` and `Candidate::sign_and_retain` are **blocking local APIs**.
  Call them under the service's bounded blocking executor, never on a Tokio
  reactor. `sign_and_retain` owns a private runtime for the frozen async validator.
- Target/profile/delegation objects come from protected provisioning. Requests
  contain references, not those objects. A `Candidate` has private fields and no
  key input; only the final signer receives the **router** key and a public,
  coglet-key-signed delegation. No build API accepts a coglet private key.
- The signed delegation must match the referenced ID, router, target and runtime,
  allow `hub.git.push`, be unexpired, and match current shared `Authority` state.
  Recheck after validation; missing clock, revoked/changed delegation or a missing
  validator fails closed. These checks are not an atomic authorization lease.
- Build in a fresh private temporary Git repository; the source checkout is not
  changed. Source build scripts and repository hooks never execute. All Git
  processes use a fixed executable, clean environment, disabled hooks/transports/
  replacement objects and bounded output. Linux supervision applies CPU, address
  space, output-file and wall limits, retains the unreaped group leader through
  group termination, and refuses unsupported platforms before spawning.
- Commit source/manifest, exact `bin/step.wasm`, retained recipe/toolchain locks,
  publication policy and canonical `build/provenance.cbor`. Retain original and
  rewritten modules and transformation-tool bytes for reproduction. The recipe
  includes the actual Cargo.lock and rust-toolchain.toml bytes, not just labels.
- Run the **unchanged coglet-proto bundle validator** on the exact bundle using
  temporary validation-only metadata under an ephemeral, non-router identity.
  Only after fsck/tree/archive/Wasm/provenance verification and current delegation
  recheck may the real router key produce the final `hub-ref` signature.
- `coglet-hub-build-validator` is a package-local launcher for the frozen helper,
  not another implementation. Its absolute installation path is trusted config.
- Return a dispatchable artifact only after bundle, signed metadata, exact inputs,
  intent digest and audit commit in the existing Store's one SQLite transaction.
  The store uses WAL/FULL durability. UID/operation-ID retries return the exact
  retained bytes; changed target/profile/delegation/source/generation intent is a
  conflict. Retrieval after expiry does not confer new effect authority.
- Limits: 64 KiB each source/manifest/module, 1 MiB retained metadata, 64 MiB bundle,
  64 artifacts and 128 MiB aggregate logical artifact bytes. No eviction. Quota or
  audit failure returns no artifact. UPDATE, DELETE and REPLACE cannot rewrite a
  retained row. Quotas bound logical payload, not physical SQLite/WAL overhead.

The saga must retrieve retained custody before rebuilding after restart. Immediately
before **every new effect**, recheck current enrollment, exact actor/ticket grants,
delegation generation/expiry/revocation, promotion policy/gates, ref CAS and kill
switch. An old artifact or a valid signature is never a lease. This module neither
dispatches nor claims an applied/running runtime; it cannot substitute for the real
Git/install/start saga and its original-scope receipt reconciliation.

## Named tests

```sh
cargo test --locked -j 2 -p coglet-hub
cargo clippy --locked -j 2 -p coglet-hub --all-targets -- -D warnings
cargo fmt -p coglet-hub --check
python3 crates/coglet-hub/run_conformance.py
```

`tests/build.rs` names real compilation/provenance, frozen verification/signing,
byte-identical **fresh subprocess** recovery, immutable retries, tampered
source/manifest, wrong profile/output pin, floats/SIMD/ambient imports, expired/
missing/revoked delegation/clock/helper, revocation during validation, source-script/
hook key isolation, REPLACE refusal, capacity without eviction, and audit rollback
with exact retry. Private supervisor tests cover a hung child and output overflow.
The conformance runner remains the unchanged **10-case real-iroh event-kernel
subset**, not new build-wire conformance or end-to-end deployment qualification.
