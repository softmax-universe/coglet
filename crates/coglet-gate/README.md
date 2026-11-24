# Native sandbox evidence

`python/coglet_gate` implements the ADR-0019 sandbox service using the native
`cogletd`/WAMR evaluator in `coglet_sim.evaluation`. The Rust library remains
unused; this service does not introduce a second router or promotion database.
Run in the sim environment (`uv run --project sim --locked ...`).

## Authority boundary

**This implementation authenticates testbed artifacts; it does not qualify a
production build authority.** `BuildIssuer.fixture` always marks evidence
`fixture-only`, including the exact digest of supplied fixture provenance.
It does not assert a Git checkout built the executable, validate arbitrary
provenance as a production build statement, or authorize deployment.
`GateVerifier.verify_production` unconditionally refuses. The only admission API
is explicitly named `accept_fixture`. There is no flag to relabel a fixture as
production. M2 fixture provenance is still rejected as production authority.

Operators supply **separate** build and gate HMAC keys (at least 32 bytes each),
pin their identities and pin native tool/adapter digests independently. Nothing
loads keys, identities or binary paths from candidate/factory output. HMAC means
symmetric trust: a verifier holding the gate key can forge gate signatures, and
must be trusted as much as the sandbox issuer. This is authenticated local
storage, not public-key attestation or verification across mutually distrustful
services. Key rotation invalidates old signatures. No credentials are written
to artifacts. Code with signing keys is inside the authority boundary.

`ArtifactStore` publishes complete canonical JSON with an atomic no-replace hard
link, fsyncs file and directory, and addresses it by SHA-256 of the entire signed
envelope. Repeating the exact write is idempotent. Reads check content address,
canonical bytes, issuer, artifact kind and domain-separated HMAC. File modes are
not the security boundary; filesystem mutations are refused on read, even if an
administrator changes permissions. Authority-owned directories, challenge DB,
keys and installed Python environment must not be candidate-writable.

The service does **not** install OS isolation. Run it in an independently
restricted sandbox with no live-world bindings. Native tools use fresh private
ledgers and Python-SDK adapters. The evaluator runs the exact rewritten Wasm
bytes and independently reexecutes the native journal and the world trajectory.
Infrastructure/replay failures raise and issue no report; behavioral failures
produce authenticated rejected evidence. Cartpole must survive the configured
step horizon, Gridbot must reach its goal within that horizon, and both require
the configured minimum action count. This is fixed-point Cartpole, not Gym parity.

## Local API and exact bindings

1. Construct `Binding(coglet, repo, candidate, base, revision, rewritten_sha256)`.
   Candidate and base are full same-format Git IDs; revision is the existing
   32-hex policy revision. Repository is an opaque registry identifier, not a URL.
2. `BuildIssuer(store, build_key).fixture(binding, rewritten, provenance)` refuses
   wrong rewritten bytes and creates the immutable authenticated **testbed** build
   artifact. It does not inspect source or promote fixture provenance.
3. Construct `Suite(suite, world, seed, max_steps, minimum_actions)`. Configuration
   is closed and integer-bounded. The seed is used by **both** native guest
   scheduling and the world. Gridbot has a bounded evaluator horizon too.
4. Construct `GateService(store, build_key, gate_key, NativeTools(...))` with pinned
   absolute executable paths. Its identity covers both native binaries, adapter/
   evaluator/SDK Python source, the ABI setup fixture and Python version. This is
   not a hermetic dependency attestation; the operator owns the environment.
   Native binaries are copied privately and digest-checked before execution.
5. Construct `GateVerifier(database, store, build_key, gate_key, tools=pinned_tools)`.
   `challenge_fixture(binding, suite, build_ref)` persists a unique authenticated
   random challenge binding every field above plus exact tool identities. A
   missing tool pin is refusal, not wildcard trust. Never learn trusted pins from
   an untrusted report. Tests construct local service pins only as a test fixture.
6. `service.evaluate(work_directory, challenge, rewritten)` runs the actual native
   episode and returns an immutable evaluation reference, binding the complete
   request, build ref, tool identity, report and qualification. Callers cannot
   supply `passed` or a precomputed report to this API.
7. `verifier.accept_fixture(challenge, evaluation_ref)` authenticates both build
   and evaluation, checks exact configuration/reexecution and atomically consumes
   the exact stored challenge. Passing **and rejected** results are consumed.
   A second consumer or restart replay is refused. This is a one-use admission
   primitive, **not** the router's repeatable `GateAuthority.verify` hook.

Challenge storage is gate-only SQLite WAL/FULL; it is not promotion state and
contains no router policy tables. Protect and retain the DB: deleting/restoring
it is outside the replay threat boundary (an unknown nonce still fails closed).
No age/freshness clock is claimed here. The later router integration must retain
an admitted evidence association, pin its trusted observation clock, and enforce
policy freshness again at propose/promote/dispatch, never repeatedly consume a
nonce or treat a passing fixture report as production authority. No production
startup, capability grant, activation or `policy-v0` publication is enabled.

## Named verification

`bash sim/check-native.sh` builds the real daemon, proof executable and compiled
rewritten Rust pilot, then runs the existing native tests and
`sim/tests/test_gate_artifacts.py`. Hosted Rust CI already runs this script.

- `test_native_authenticated_gridbot_cartpole_pass_and_reject`: four real native
  episodes (both worlds pass/reject), independent replay, artifact acceptance,
  replay refusal and mutated executable refusal. Gridbot rejects a one-step
  horizon before its goal; Cartpole rejects a horizon longer than actual survival.
- `test_artifacts_are_content_addressed_and_tamper_refused`: immutable/idempotent
  publication and mutated authenticated build artifact refusal.
- `test_issuer_refuses_wrong_bytes_and_fixture_never_becomes_production`: exact
  bytes, no production path, even when a trusted test signer changes a label.
- `test_build_exact_binding_refuses_transfer`: target/repo/candidate/base/revision/
  executable binding matrix.
- `test_verifier_authentication_challenge_binding_and_restart_replay`: wrong key,
  wrong kind, suite/seed/config transfer, different challenge and restart replay.
- `test_invalid_bindings_and_suite_fail_closed`: closed lexical and integer bounds.

Additional negative/concurrency tests are listed in the PR with actual results.
The synthetic report in the verifier unit test is explicitly a fixture for that
trust boundary, never native execution evidence. Only the four native episodes
qualify the native evaluator claim.
