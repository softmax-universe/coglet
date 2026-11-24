# ADR-0025: Protected local R3 transform publication profile

- Status: Proposed
- Date: 2026-11-24
- Authors: H-transform qualification owner, task `t_01a20d98a7fb7229`
- Related PR: recorded in Approval when opened
- Supersedes / superseded by: none; supplements ADR-0003 and ADR-0023

## Context

[ADR-0023](0023-m1-residual-withheld-routes.md) withholds H-transform publication
until an actual rewrite, pinned differential/cross-architecture execution and
authenticated admission exist. It does not select a new publication interface.
The [frozen build contract](../../spec/hub-protocol.md#build-rewrite-sign-and-replay)
already requires original/output retention, gates on the rewritten bytes and
signed output-bound provenance. Identity fixtures demonstrate none of the missing
positive behavior. The existing reference-builder proposal (`integer-WAT-v1`,
PR #121) deliberately excludes floats and is not R3 qualification.

The orchestrator reserved 0025 and selected the scope in rulings 247995/247997.
This ADR proposes a **protected local** publication profile and its qualification
boundary. It adds no iroh methods, changes no frozen schema/identifier, and does
not accept all of ADR-0003 or qualify any engine's resource cutoffs.

## Decision

### Profile and pure rewrite seam

Add opt-in `r3-scalar-nan-v1` alongside `integer-WAT-v1`. The integer profile stays
the default and unchanged. Profile selection is trusted local configuration,
not a field an untrusted builder can use to grant itself publication authority.
Until qualified, selecting the new profile is refused before build signing,
ref mutation, transfer or activation.

Implement an independent pure module on merged main: exact original core-Wasm
bytes plus pinned profile/tool parameters yield transformed bytes and a digest
record, or a refusal. It uses no signing keys, network, mutable registry or clock.
The builder remains the sole owner of compilation, signing and Store/ref custody.
Wire the pure module into that builder only after the builder is on main; do not
copy its custody logic. Owner `t_01a20bcfe6db20c3` coordinates that seam and shared
CI. Full deployment acceptance and `hub.receipt` are not module prerequisites.

The profile applies ADR-0003 R1/R2/R4/R5 admission (including the frozen R4 ABI
allowlist/signatures) before rewriting, then validates the output under the same
feature set. Reject unknown operators, unsupported sections/features, shared
memory, atomics, SIMD/relaxed SIMD and forbidden imports rather than copying
unexamined executable instructions. A deliberately narrower supported module
subset must be explicit in the implementation's profile document and negatives;
it cannot advertise general R5 support. Malformed input, growth beyond configured
finite byte/operator/local limits and validation failure are refusals.

For both f32 and f64, rewrite every ADR-0003 R3 result-producing operation:
add, sub, mul, div, sqrt, ceil, floor, trunc, nearest, min, max, and the applicable
promote/demote conversion. Immediately test the computed result for NaN and
substitute positive quiet NaN bits `0x7fc00000` / `0x7ff8000000000000`; otherwise
retain the exact result. Locals introduced by the rewriter must not alias original
parameters/locals. Preserve traps and evaluation order. Leave bit-exact neg, abs,
copysign, constants, loads, stores and reinterpret operations untouched, including
payload-bearing NaNs copied through state. Do not normalize state hashes or rely
on an engine canonicalization flag. A module with no affected operators may have
identical original/output bytes; a float-positive test must actually change bytes.
The tool's exact artifact digest and profile bytes, not a version label alone,
identify the transform. Rewriting twice is not the build pipeline.

### Retained evidence and authenticated admission

The pure module's digest record is **not a credential**. Retain digest-addressed
original and output modules, transform artifact/source revision, policy parameters,
compiler/recipe and dependency lock, corpus generator/revision/seed and corpus
bytes, and the full per-mode gate outputs. Use SHA-256 for byte identities. Each
evidence record binds these pins and the exact original/output digests. Gates
execute only the output later placed at `bin/step.wasm`; retain the existing
closed `hub-build-record` schema unchanged. Extra evidence is retained separately,
not smuggled into that closed record or substituted for its signed archive.

A trusted local qualification authority is configured separately from untrusted
build input. Its allowlist names exact transform, profile, toolchain, corpus,
engine build/configuration and execution-mode pins qualified by successful required
CI. Never trust a caller's `qualified: true`, self-signed evidence or a digest with
no authenticated issuer. CI evidence must be fetched/verified through a trusted
CI identity and exact successful run/head/job/artifact binding before the local
authority approves those pins. Test keys/fixtures never populate production trust.
Native and emulated execution are distinct modes; no unexecuted target, engine,
compiler option or toolchain revision inherits qualification.

The existing router signing authority authenticates the admitted publication
through its signed ref/archive. Before it signs, a protected local admission
check must match the allowlisted qualification pins and verified retained evidence
to the exact source commit, original/output, profile, recipe and provenance. Bind
the admission to authenticated publisher and router identities, target coglet and
runtime/device identity, current delegation generation/digest and ref operation
(old tip, proposed new tip and generation). The publisher identity is the existing
authoritative writer for that ref, never a new builder key. A publication-scoped
authenticated admission statement covers these fields and the evidence digest;
its trusted issuer is explicitly provisioned, not accepted from the request.
Unsigned, altered, foreign, expired, revoked or stale-delegation statements fail.
At the final existing signing/ref-CAS boundary, recheck current authority and all
bindings; an admission token cannot authorize a different operation after a race.
No original-module hash alone authorizes execution. The coglet's `published-ref`
retains the admitted archive unchanged; it cannot rebuild or launder provenance.

No new wire credential is required: these are protected local inputs to existing
build/sign admission. Consumers retain the frozen archive/signature/provenance
checks. This ADR authorizes implementing this local profile/interface, not bypassing
delegation, grants, gates, CAS or any independent runtime admission.

### Required execution and refusal gates

All five names below are **unexecuted implementation obligations**, not receipts
from this docs PR. Required CI must execute them with zero skipped/ignored cases:

- `rewrite_before_build_commit`: real affected arithmetic changes the module;
  gates see those exact bytes before signed build/commit. Original/output and all
  pins are retained. A rewrite/validation/gate/admission failure leaves refs and
  live code unchanged. Include a no-affected-op control and sign/commit ordering.
- `unqualified_transform_publication_refused`: absent/mismatched qualification,
  unsupported architecture/toolchain/engine mode and identity-only evidence never
  reach signing/ref mutation; the unchanged integer profile remains usable.
- `pinned_transform_fuzz_differential`: a reproducible pinned generated corpus
  exercises every rewritten opcode and width with NaN payloads/signs, infinities,
  signed zeros, subnormals and ordinary finite values. Compare executed output
  against an independently expressed R3 oracle, including raw bits and state
  hashes, traps and payload-preserving bit operations. Do not compare only two
  outputs of the same rewrite. Retain failing seeds and full corpus/output hashes.
- `pinned_transform_cross_arch_replay`: execute identical transformed bytes and
  recorded inputs on x86_64 and aarch64, using pinned WAMR interpreter and Wasmtime
  configurations. Compare raw per-step state hashes and bit results with each other
  and the independent oracle; pair evidence only when all source/output/corpus/
  toolchain/engine pins match. A cross-compile, source scan, identity guest, cached
  fixture or architecture label is not execution. AOT and other unexecuted modes
  remain refused. Record whether each runner is native or emulated explicitly.
- `authenticated_transform_publication_admission`: exercise real signature and
  authority verification with a valid positive plus missing/forged/altered issuer,
  publisher/router/target/runtime/source/output/profile/evidence mismatches,
  expired/revoked/stale delegation and ref-race/replay negatives. Verify the signed
  archive's unchanged provenance and output binding through existing admission,
  not just a signature helper returning success.

Required matrix aggregation fails on missing jobs, missing test names, zero-test
runs, skips, differing evidence pins or absent retained outputs. CI records exact
source head, runner architecture/mode and engine/toolchain digests. An aggregate
which depends on both architecture legs must be a required merge-queue check;
workflow edits are minimal and coordinated with the shared-file owner. Gates
also run for `merge_group`, not only PR heads. This is bounded qualification of
the pinned profile/modes/corpus, not a mathematical proof for every Wasm program.

## Alternatives

- Treat signed identity provenance as R3 proof: rejected; it binds bytes but never
  exercises canonicalization or architecture behavior.
- Extend the integer profile silently: rejected; it removes the builder's explicit
  admission boundary and changes existing defaults.
- Add a publication RPC or duplicate signing/Store registry: rejected; the seam is
  local and the frozen ref/archive protocol already supplies custody.
- Copy the spike rewriter as product code: rejected by plan ground rule 1; the
  implementation is clean, with spike cases used only as test ideas.
- Engine flags or canonicalized state hashes: rejected for ADR-0003's reasons;
  neither enforces R3 in the guest's computation on every admitted engine.

## Consequences

The new profile remains fail-closed until real CI and authenticated integration
prove the named obligations. This ADR does not mark H-transform qualified. The
ledger may name bounded qualification only after exact-head CI, independent
review and merge-queue landing of implementation and builder integration. Engine
E-0003, all-method router conformance and product deployment retain their owners.
A change to admitted features or trusted pins needs fresh relevant evidence; an
unexecuted mode remains withheld, never an ignored passing test. Revoking local
qualification disables new publications without rewriting previously admitted
archives or granting authority to execute them on a newly unsupported runtime.

## Verification

This docs-only proposal runs required docs/link checks and independent contract
review. The future implementation must supply the five named tests above and
retained exact-head CI artifacts for both architectures with zero skips. It must
retain existing provenance/delegation/ref-CAS and integer-profile regressions.
No spec, production code, publication interface or CI workflow changes accompany
this proposal. Implement those only after this ADR is independently reviewed and
merged through the queue.

## Approval

Proposed for independent exact-head review and merge-queue acceptance. This text
claims no review verdict, successful positive test execution or qualification.
