# Immutable Git tree hashing

Task: t_01a2084f99c437b8. No ABI, identity, budget or authority changes.

Current ABI Git SHA-256 tree identity is computed directly over the validated
immutable decoded filesystem. Each blob lazily caches its Git object ID (not its
existing raw content digest). Copy-on-write map clones share the cache; replacing
bytes replaces the blob/cache. Trees remain rebuilt with Git's directory-aware
name ordering. The public lowercase-hex map entry point retains complete path,
hex and collision validation. The runtime native path uses its already validated
exact decoded snapshot; legacy CBOR and frozen hub0 hashing remain unchanged.

## Named checks

- `git_blob_id_cache_is_lazy_and_shared_across_copy_on_write`: cache is lazy,
  unchanged blobs share it across COW mutation, changed blobs do not, old snapshots
  retain their exact hashes.
- `cached_git_tree_matches_hex_reference_for_prefix_order_and_empty_blobs`: direct
  and hex paths agree for empty roots/blobs, Unicode and Git prefix ordering;
  raw blob digest is unchanged.
- `state_hash_matches_git_sha256_tree`: both paths match actual Git SHA-256
  `write-tree` for every existing oracle case.
- `invalid_snapshot_is_not_hashed` and `state_hash_rejects_unmappable_paths`:
  uppercase/nonhex/odd hex, invalid roots, .git and file/directory collisions refuse.
- Runtime `snapshot_hash_cache_preserves_wire_equality_and_invalidates_mutations`
  and `incremental_fs_projection_matches_full_export_on_changed_code_and_data` cover projection,
  guest writes, host logs and restored exact wire equality.

## Measured scope and limits

An isolated diagnostic clone instrumented Runtime::step phases on the archived
native-avatar prefix (13 steps), replaying the actual pinned compiled guest with
new live chain hashes. All 13 historical output hashes agreed before and after.
The same separate release target was rebuilt after only this optimization.
Pending steps 1–12: baseline mean 70,565.67us, maximum 81,863us; optimized mean
56,192.67us, maximum 75,599us. These are single diagnostic runs, not statistical
performance guarantees. The record/tree phase commonly drops from ~18ms to <1ms;
other phases and scheduling still exceed the total 50ms envelope in this sample.

This is NOT actual native-map acceptance: no live game or blocked Gateway worker
ran in this diagnostic. The real product remains pinned to merged source with
failed actual native qualification at 67,047us against the unchanged 50,000us gate.
Landing this change does not certify that gate. Required CI, independent exact-head
review and actual merged-product rebuild/native-map/replay proof remain mandatory.
