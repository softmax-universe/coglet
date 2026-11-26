# Router (hub protocol 1.0) fixture catalog

- `cases.json`: every local CDDL/JSON Schema rule has a positive golden CBOR
  vector; closed maps, domains and enclosing roots have shape negatives. Shape
  validity is not proof of signer, authority, code admission or temporal behavior.
- `framing.json`: malformed u32-prefixed/non-profile frames; all reject.
- `signatures.json`: actual test-key Ed25519 delegation signature and exact
  domain-separated preimage. `test_signing_vectors_and_target_binding` verifies
  it with OpenSSL, plus patch signatures and altered target/generation controls.
- `patch.json`: a signed filesystem Patch, resulting `/code` map, pinned source
  tree and test-only no-op build provenance. The empty core Wasm is a digest
  binding fixture, not guest ABI or ADR-0003 transform qualification.
- `patch-rejections.json`: correctly signed stale-base, equal/lower-generation,
  unsafe-path and missing-executable candidates; executable rejection has the
  correct recomputed resulting revision. `test_patch_stale_base_replay_bad_path_missing_executable`
  checks typed errors and no input mutation. `test_patch_oversize_boundary`
  tests exact 64 MiB admission and an actual encoded overrun without committing
  a large fixture file.
- `tail-restart.json`: committed ordered log-entry bytes and a persisted cursor.
  `test_tail_across_fresh_process_restart` crosses an actual interpreter boundary
  with only persisted bytes/cursor, then resumes without duplication or omission.
  It is an archive contract oracle, not a live runtime durability claim.
- Each non-codec JSON vector has an adjacent `.json.format` marker containing
  `semantic-vectors-v1`. Codec exclusions are counted; protocol tests own these
  vectors explicitly.

Regenerate from the repository root:

```sh
python3 spec/messages/hub/generate.py
python3 spec/tests/build_manage_vectors.py
python3 spec/tests/generate_abi.py
python3 spec/tests/build_boundary_receipts.py
python3 spec/tests/build_mc_contracts.py
python3 spec/tests/build_database.py
codegen/run
```

Run `python3 -m unittest discover -s spec/tests -v` with the pinned Python/Ruby
requirements. Generated ABI timeline companions for the new method/provenance
names live in `spec/fixtures/abi/*-timeline.fixture`. The old `conformance/`
implementation corpus is unchanged here; its runtime conversion is separate.
No runtime, CLI, mock or registered 1.0 implementation is qualified by these
specification oracles.
