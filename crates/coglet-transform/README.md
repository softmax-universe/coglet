# coglet-transform

Pure `r3-scalar-nan-v1` rewrite and **local admission component** (ADR-0025).
`integer-WAT-v1` stays the default. This crate does not sign build archives,
mutate refs, or qualify H-transform. Builder wiring stays with
`t_01a20bcfe6db20c3` after that builder is on main.

## Evidence boundaries

- `rewrite_component_changes_arithmetic_and_preserves_identity` proves byte
  rewrite, engine admission and output binding only. It does **not** implement
  `rewrite_before_build_commit`; no signing, Store or ref assertions are faked.
- `local_admission_signature_authority_expiry_revocation_and_ref_binding` uses
  explicitly labelled test-only fixture evidence and keys. It proves local
  cryptography/binding/refusal only, **not**
  `authenticated_transform_publication_admission` through a signed archive.
- `unqualified_transform_publication_refused` is a component refusal. The
  unchanged integer builder is neither invoked nor authorized by it.
- `pinned_transform_fuzz_differential` executes a bounded generated corpus on
  WAMR interpreter and Wasmtime with an independent host-arithmetic R3 oracle.
  `pinned_transform_cross_arch_replay` pairs full identical corpus/output bytes
  and raw results from **native x86_64 and native aarch64 required-CI legs**.
  The required `rust` job fails when either leg or its exact test inventory,
  source/run/artifact pins, toolchain or paired bytes are missing or disagree.
  Raw result bytes and their SHA-256 digests are retained; these are **not**
  runtime journal or filesystem state hashes. AOT/unexecuted modes stay withheld.
  Earlier QEMU 8.2.2 trials internally SIGSEGVed on the expanded corpus (both
  max and cortex-a72 CPU modes). No emulated mode is admitted or reported green.
  The native pairing is an implementation obligation until its hosted CI runs
  actually finish; local x86_64 success alone is never a cross-arch receipt.

`check.py` directly executes the exact test inventory (outside the quarantine
wrapper), rejects missing/renamed/ignored/zero/duplicate/failed tests and retains
corpus/result bytes, logs, actual checkout/run identifiers, exact executed native
test binaries, toolchain and lock bytes. Required `rust` depends on both native
legs and runs the exact one-test aggregation inventory, including `merge_group`.
Same-run artifact acquisition uses Actions upload/download custody, not an
arbitrary URL. The component artifacts are review inputs, not automatically
trusted production publication credentials.

The protected-local `Allowlist` must be provisioned **only after** trusted CI
identity and successful exact head/run/job/artifact verification. There is no
CI-fetch/provisioning implementation here and no helper which treats matching
caller-provided CI strings as authentication. `Evidence` verifies retained bytes
against every pin and the signed publication scope; `Authority` is a fresh trusted
input, never request data. No production trust defaults or test keys are supplied.

Remaining ADR-0025 integration obligations are intentionally open: authenticated
CI acquisition/provisioning, actual builder rewrite→gates→sign→Store/ref-CAS
ordering, replay/ref-race fencing at that final boundary, retained signed archive
provenance verification, and runtime state/trap/payload-chain proof. Do not mark
the ledger qualified or deploy this module as a publication path until these run
in required CI, receive independent review and land through the queue.

Local component verification on either native architecture:

```
python3 crates/coglet-transform/tests/check_runner.py
COGLET_TRANSFORM_EVIDENCE_DIR=/new/private/output \
python3 crates/coglet-transform/check.py --native-leg
```

Aggregation requires both real same-run architecture artifacts under
`COGLET_TRANSFORM_NATIVE_PAIR` and the original Actions run/attempt context.
Do not relabel local output as another architecture to make it pass.
