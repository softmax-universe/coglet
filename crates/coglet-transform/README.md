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
  `pinned_transform_cross_arch_replay` executes the same corpus/output bytes on
  x86_64 native and aarch64 qemu-user using both engines. Raw result bytes and
  their SHA-256 digests are retained; these are **not** runtime journal or
  filesystem state hashes. Native aarch64/AOT/unexecuted modes are not qualified.

`check.py` directly executes the exact test inventory (outside the quarantine
wrapper), rejects missing/renamed/ignored/zero/duplicate/failed tests and retains
corpus/result bytes, logs, actual checkout/run identifiers, exact executed test
binaries, aarch64 peer, emulator, toolchain and lock bytes. CI runs this in the
required `rust` job, including `merge_group`, and uploads the complete manifest.
The component artifacts are review inputs, not automatically trusted credentials.

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

Local component verification (Linux x86_64 with configured aarch64 peer):

```
python3 crates/coglet-transform/tests/check_runner.py
COGLET_TRANSFORM_PEER_EVAL=/absolute/peer-wrapper \
COGLET_TRANSFORM_PEER_ARCH=aarch64 \
COGLET_TRANSFORM_PEER_MODE=emulated-qemu \
COGLET_TRANSFORM_EVIDENCE_DIR=/new/private/output \
python3 crates/coglet-transform/check.py
```
