# Frozen router 1.0 contract

PR #164 merged with CBOR filesystem/code-tree identity before the ADR-0035
implementation. Per ADR-0035's fallback, that meaning is preserved here; the
active router spec and generated SDK contracts are **2.0**, not a redefinition
of 1.0. `protocol.md`, `hub.cddl.frozen` and `hub.schema.json.frozen` are verbatim
snapshots of the merged normative text and shapes. The `.frozen` suffix keeps
them out of the active schema compiler; they are not active generated families.

For 1.0 only, filesystem identity is SHA256(core-deterministic-CBOR(snapshot)),
and code identity applies that function to absolute `/code/...` paths. The 1.0
build provenance record has `version: "1.0"`. Router 2.0 uses Git SHA-256 root
tree ids for those fields and `version: "2.0"` build records; it cannot silently
accept a 1.0 selection or reinterpret an old build record. Non-filesystem hashes,
including patch-transfer digests, signatures and recipe/Wasm digests, retain
their original definitions.
