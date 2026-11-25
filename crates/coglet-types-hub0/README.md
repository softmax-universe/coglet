# Frozen hub/0 implementation contract

This private package preserves `src/generated.rs` and
`fixtures/signatures.json` byte-for-byte from Coglet commit
`1e972a7c2f4071af346ba5dc45da1ec6cb7f2f01` (the signatures were formerly
`spec/fixtures/hub/signatures.json`). Its validator/codec `src/lib.rs` is copied
from that commit with only visibility and unchanged database-type re-exports
added: the guest database SDK and old implementation must share Rust type
identity. It is not a new generated-code target.

ADR-0031 changes the authoritative spec and generated `coglet-types` to hub/1.
The existing proto, runtime, hub and CLI implementations have not migrated.
Their dependency aliases explicitly pin this older contract so that signature,
provenance, grants and message checks remain strict and their old conformance
suite remains executable. No old Git method is accepted by the hub/1 codec.

Do not regenerate this snapshot from current `spec/`, weaken validation, or
advertise legacy implementation tests as manage/hub/1 runtime qualification.
Remove these dependency pins and this package when the separate implementation
migration is complete. Common/surface/ABI contracts shared by both revisions
remain subject to their ordinary validation suites.
