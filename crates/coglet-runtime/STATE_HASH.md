# ADR-0035 state-identity qualification

Filesystem identity is the SHA-256 Git tree object ID, implemented once in
`coglet-fs::snapshot_tree_hash`. ABI records/plain manifests are 1.0; database
records/manifests are 1.1. Non-filesystem hashes and record fields are unchanged.

Historical WAL admission is private to recovery. A validated old segment keeps
its original records and hash algorithm. Recovery seals it and starts a new
versioned segment atomically; an unresolved attempt postpones the cut until
reconciliation. It never rewrites an old record or re-enters the guest.

| Requirement | Named test / executed gate |
| --- | --- |
| Exact Git framing, modes, raw IDs, nested/order/binary/empty snapshots | `coglet-fs::state_hash_matches_git_sha256_tree` (actual SHA-256 Git repository) |
| Case-insensitive `.git` and file/directory collision refusal | `coglet-fs::state_hash_rejects_unmappable_paths` |
| One production implementation | runtime `state_hash_single_implementation` |
| Old CBOR-named disk trees, restart, corruption and stale writer fencing | fs `disk_migration` integration target |
| Old WAL seal/new successor and reopened trace boundary | runtime library `disk_store_upgrades_cbor_named_trees` |
| Uncertain attempts remain fenced, no guest re-entry | `hash_upgrade_defers_unknown_attempt_then_reconciles_without_guest_reentry` |
| Real pending SQLite decision resolution before exactly one successor | `hash_upgrade_recovers_old_database_prepared_commit_before_single_cut`, `hash_upgrade_recovers_old_database_prepared_abort_before_single_cut` |
| Exact old database record preservation | `hash_upgrade_database_keeps_exact_historical_record_bytes` |
| No public legacy admission | `hash_upgrade_public_paths_reject_historical_versions` |
| Cut discriminator, empty history, staged mutations/queues/blobs | remaining `hash_upgrade_tests` library cases |
| Restricted code snapshot includes `/code` root | spec `code_tree_hash_is_restricted_snapshot_id` |
| Independent engine and CLI replay identities | Wasmtime `cross_engine_state_hash`, `engine_trait_conformance`; CLI `cli_build_run_replay_equal_hash_list_and_inspect` |
| Frozen debug0.1 CBOR identity is distinct from current ABI Git identity | `frozen_debug_hash_stays_cbor_while_current_abi_is_git`; receiver25 unchanged cases / independent oracle |
| Frozen serving Git-transfer artifacts remain available | runtime `tests/data/hub0/fetch.json`, byte-identical historical blob, receiver fetch cases |
| Full runtime corpus and actual archived guest replay | `tools/check-abi.py`: 170 harness cases, zero skips; 5 named WAMR archive tests / all 32 guest records |
| Runtime generator freshness, no spec corpus mutation | spec `test_runtime_abi_corpus_fresh_and_spec_unchanged` |
| New observed visualizer traces, not old trace conversion | `tools/visualizer/regenerate_state_hash_fixtures.py`; visualizer/native/browser tests |

The current spec's router hash revision is **2.0**, following ADR-0035's
next-major fallback after router 1.0 landed with CBOR identity. Frozen 1.0
contracts are retained under `spec/frozen/hub-1.0`. The serving runtime still
uses its separate private hub0 router codec; current ABI manifests/records use
`coglet-types-current`. Runtime timeline generation deliberately preserves that
frozen router provenance and debug naming while using current ABI identity.
This card does not implement a serving router 2.0 migration.

Qualification above is host/runtime/engine and deterministic archive proof, not
physical-device or iOS sandbox qualification. Socket/C-ABI negotiation is a
separate small role-selected corpus, not full operational proof. Hosted required
CI and independent exact-head review remain mandatory before native queue entry.
