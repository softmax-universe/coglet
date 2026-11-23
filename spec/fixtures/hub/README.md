# Hub fixture catalog

- `cases.json`: shape-valid/shape-invalid JSON views, selected CDDL rule and
  JSON Schema definition, and core-deterministic `cbor_hex` for each positive.
  `valid` means **schema shape**, not enrollment, crypto, grant or temporal
  acceptance. In particular the repetitive keys/signatures in shape fixtures are
  not signed credentials; use `signatures.json` for actual signature verification.
- `framing.json`: full u32-prefixed malformed/non-profile wire frames. All reject.
- `signatures.json`: actual test-key Ed25519 signatures for four signing domains,
  including exact signed CBOR preimages. Domain prefix is not part of the CBOR
  item. Tests verify signatures and reject changed preimages with OpenSSL.
- `git.json`: actual Git bundle v2 plus archive, SHA-256 digests, object ID and
  hub-key signed ref statement. No private `/data` files. Tests import into an
  isolated repository, run strict fsck, reconstruct the archive and verify the
  signature. This is a transfer fixture, not a guest implementing the Wasm ABI.

Schemas import `common.cddl` / `common.schema.json`; generators live in
`spec/messages/hub/`. Run from repository root with the fixture dependencies
installed, as described in `conformance/hub/README.md`. Shape checks do not
replace the shared future wire harness or its required stateful oracles.
