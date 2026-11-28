# Stage 2 implementation plan — not qualification evidence

Baseline Coglet ec31e29d4c72f43fe30b716e02012bacb0e36699; consume Unforged
f9c627bcc09346b2923583037a10ac9a2fa8ea12. Follow AGENTS.md, docs/plan.md,
staging and ADR-0042 §4. No new scheduler, router store, device Git, production
build authority, or effect-path replacement. Frozen contracts remain unchanged.

## Existing inventory and observed baseline

`existing-test-inventory.json` lists exact source names, per-file SHA256 and
counts: 95 relevant files, 453 source test definitions (not collected parameterized
cases), expanded to include the entire runtime integration-test subtree before
runtime/gate implementation edits. The initial narrower inventory was 26 files /
122 definitions; it omitted execution/database/factory modules that must remain
covered. Baseline `python3 -m unittest discover -s tests/unforged_proof -v`
executed all four stage-1 guard tests successfully. None is hosted qualification.
Retain every existing test. Unforged's synthetic Coglet engine and receipt mocks
are not a substitute for a real recorded cogletd step.

## Independent plan review and correction

First independent plan review requested changes (separate session
`sa-0-1bc5776f`): specify the factory seam, invocation binding, independently
issued private authority, real fork oracle and worker-side no-effects controls.
The implementation below addresses these; obtain corrected-plan approval before
editing implementation. Exact-head implementation review is a separate gate.

### Recording, replay and fork

Extend existing `run.py` and `driver/src/main.rs`, retaining the counter control.
Record an effectful synthetic guest with genuine `cogletd --sandbox-stdio`, using
existing runtime journal, inbox and recorded model/adapter outcome mechanisms.
Provider outcomes are deterministic fixtures explicitly identified as such; step
execution and its resulting trees are real, not expected-output emulation.

For replay, use existing `Runtime::from_fixture`, `Runtime::replay` and the
prefix-import/single-native-step driver as appropriate. Transfer only recorded
inputs/complete required replies, setup and the earlier prefix; no current/future
output snapshots or expected tree in a worker's executable input.

For fork, use existing native visualizer continuation logic
(`tools/visualizer/native/src/main.rs::fork`) and Runtime segment operations,
not a renamed replay. First execute the edited-input continuation independently
with real cogletd/native engine to obtain the reference trace. Compare every
hosted step's entire tree against that trace. Retain suffix-only inbox edits,
generated chain links and continuation applicability checks. Refuse when changed
actions/model request identities make later recorded replies inapplicable.
Factor reusable offline logic minimally rather than copy a second executor.

### Freshness and authenticated receipts

Use current `stream exec --no-cache` / `CachePolicy::Bypass`; stage-1 cache
ref deletion alone is insufficient. Pin resolver and native proof binaries.
Retrieve `StepReceipt` through existing `unforge coglet receipt <commit>` / its
existing notes implementation, not CLI `memo_hit` alone. Verify the receipt is
unique, for the exact returned commit, and has runner=caos, expected resolved
worker image, executed=true, cache=none, memo_hit=false. Verify returned commit
parents/tree/resolver against the submitted program and complete input closure.
Fail if any authoritative receipt readback is absent or ambiguous.

Each fresh acceptance's existing gate challenge nonce is included in the input
envelope consumed/removed by the worker. Thus an old executed receipt cannot
satisfy a new challenge even when executable/data inputs otherwise match. Bind
candidate executable, base state, full inputs/replies/boundaries, suite, policy
revision, resolver and independently expected image to that challenge/input OID.
Never derive trusted pins from candidate/factory output. Coordinator trusts the
supported authenticated CAOS channel and runner provenance, not hardware attestation
or an unsigned local receipt supplied by the factory. Warm controls repeat the
same input without bypass and explicitly distinguish local versus CAOS hits.

Exercise image/resolver changes in a single long-lived client/retained acceptance
context; separate fresh CLI processes alone do not qualify stale resolution-cache
invalidation. Add a small opt-in bridge consuming existing Unforged APIs if needed;
it belongs in the harness, not Coglet crates. Do not modify Unforged primitives.
If deployed receipts lack required bindings/capabilities, preserve refusal and
route that prerequisite rather than fabricate metadata.

### Existing factory gate seam

Add an operator-selected hosted method to the existing Python `GateService`;
keep its native default unchanged. Trusted service validates all remote receipts,
input/output trees and challenge bindings before signing its existing evaluation
artifact. Extend its pinned tool identity to cover hosted implementation and
operator configuration; factory cannot select binaries/endpoints or supply a
precomputed `passed` report. Exercise the actual sequence
`challenge_fixture -> hosted evaluation -> accept_fixture`, plus forged report,
copied receipt, stale challenge and changed-pin negatives. Retain one-use admission.

Stage-2 factory evidence remains fixture-qualified under the existing testbed
build authority. `qualification='fixture-only'` and `verify_production` refusal
must remain unchanged. This is actual hosted cold execution at the existing
factory gate, not production promotion certification. Any request to change that
authority must go back to the scope owner rather than relax it here.

### Private export and no-live-effects

Use operator-controlled authority separate from factory/candidate input. Reuse
existing gate Key/MAC trust machinery for a narrowly scoped testbed export grant,
not a new production authorization service: bind sender/recipient authenticated
endpoint identity, target coglet, operation, current grant identity and exact
exported closure digest (snapshot, prefix history, code/blobs, replies and metadata).
The operator owns authority and revocation state outside the candidate worktree.
Verify current grant before payload serialization/object writes and again before
transmission. Negative spies require zero payload serialization, object-write and
upload calls on denial; include revoked/wrong-target/wrong-recipient/content cases.
This never grants live device read/export access or widens guest grants. Only
synthetic data is used for hosted qualification.

Before remote I/O require the complete recorded-reply closure, and reject missing
or duplicate/inapplicable replies. Worker executes Runtime replay/step only, never
RuntimeService dispatchers, provider clients or live adapter mounts. Instrument
host model, adapter dispatch and guest network denial points on an effectful
recording with nonzero recorded requests/outcomes; require zero live calls and
external effects. CAOS coordinator transport/object transfer is not a replay
effect and is counted separately. Enforce worker isolation using the existing
supported sandbox, not a newly invented isolation claim.

## Test-first increments and qualification

1. Fast negatives for strict receipt parsing/binding, request/closure grants,
   missing replies, stale acceptance and no-side-effect refusals; record RED then
   GREEN. Keep source-test inventory unchanged except additive coverage.
2. Real effectful recording and replay/fork whole-tree comparison; no returned
   result projection/normalization. Implement factory hosted path and real
   challenge/admission tests within existing fixture authority.
3. Add seven named opt-in hosted tests:
   - hosted_replay_matches_recorded_step_trees
   - hosted_fork_matches_recorded_step_trees
   - hosted_factory_gate_requires_fresh_execution
   - hosted_changed_image_invalidates_acceptance
   - hosted_missing_receipt_fails_closed
   - hosted_private_export_requires_grant
   - hosted_replay_has_no_live_effects
   Missing hosted configuration is explicit unavailable/failure in the qualification
   command, never an early-return test PASS.
4. Run all three distinct entry paths, cold/warm controls, changed resolver/image,
   denied export, missing reply, missing/ambiguous/forged/copied receipts. Preserve
   raw commands/results, source/binary/resolver/image hashes, authenticated
   receipt readback and every recorded/actual per-step tree. No paid service,
   GPU, capacity provisioning or production deployment is authorized.
5. Independent exact-head review, every applicable CI check, native merge queue,
   then containing-main hosted rerun. Attach compact durable evidence and finish
   with request_review only after actual stage2 PASS.

## Concrete corrected seams and refusal semantics

- File `crates/coglet-gate/python/coglet_gate/__init__.py`: add
  `GateService.evaluate_hosted(work, request, rewritten)` using an operator-owned
  hosted runner configured on the service. Invoke hosted verification before the
  existing `ArtifactStore.put(..., gate_key, 'evaluation')` signing/publication
  call used at the end of `evaluate`. Keep the same envelope fields and put
  complete hosted bindings in its report. Extend tool identity with
  `hosted_harness_sha256`, `unforge_sha256`, `resolver_oid`, `worker_image`, and
  `endpoint_identity_sha256`; no tickets/keys in artifacts. On timeout/refusal,
  sign no artifact and leave the existing challenge unconsumed; only existing
  `accept_fixture` consumes it. Test failure/retry and no-publication explicitly.
- Actual `StepReceipt` fields are runner, worker_image, executed, cache, commit;
  it does NOT contain separate input-tree/resolver fields. Do not invent them.
  Validate *composition*: all raw receipt-note entries must be unambiguous for
  that invocation and name the exact returned commit; inspect that commit with
  replacement objects disabled, verify its tree and ordered program/input parent
  commits, inspect their trees and the resolver commit/provenance. Input contains
  nonce/candidate/base/closure/suite/revision pins. Verify this graph against the
  caller-retained request graph and expected pins, not against self-reported
  output strings. Raw `refs/notes/receipts` is authoritative, not the
  `read_latest_step_receipt` convenience API that hides earlier entries.
  Add previous-identical-data/different-nonce, swapped-output and copied-note
  negatives. The notes are trusted-client observations of authenticated CAOS
  responses, NOT independently signed server attestations. Candidate cannot
  write the trusted client's store. If that supported trust boundary or graph
  binding is unavailable in deployment, refuse and route the prerequisite;
  never claim an invented receipt field or stronger remote attestation.
- Closure authorization uses already-held validated object IDs, lengths and
  metadata descriptors; computing its grant identity does not serialize private
  payloads or write Git objects. Explicitly distinguish metadata canonicalization
  from private-payload serialization in spy counters. Sender/recipient identity
  comes from operator configuration/authenticated CAOS channel, never the
  factory request. Recheck the same grant/revocation record just before upload.
- Worker-side evidence is mandatory: deny logs/counters produced inside the
  sandbox, with a separate content-addressed diagnostics object bound to the
  invocation/returned commit (not added to the ABI filesystem tree). Include a
  deliberately attempted live-model/adapter/network canary proving enforcement
  detects/refuses, then effectful recorded replay showing zero attempted live
  calls and nonzero recorded outcomes. Coordinator transport counters alone
  cannot qualify. Use the supported existing worker sandbox/effect interfaces;
  if these cannot produce honest observation, preserve that gap rather than
  add an unreviewed sandbox implementation.

## Runtime discovery

The broker advertises `caos.prod.connect` (auto-tier), injecting only
`CAOS_IROH_TICKET`; this is the intended existing-runtime connection candidate.
Never print/persist tickets or use runner/admin credentials for client work.
Need verify supported authenticated connectivity, deployed source/image identity
and receipt capability before hosted acceptance. Default-region Hermes-account
EC2 and tailnet host listings do not inventory the CAOS account and are not
proof it is absent. No runtime has been provisioned or changed.
