# CDDL code generation

Run `codegen/run` from any directory; commit its Rust, Python, Go and Java outputs.
`codegen/run --check` is read-only and exits nonzero if any output is stale
or missing. The conformance CI job enforces this, exercises the generator and
cross-checks the generated validator against actual CDDL and JSON Schema on
all generated-contract semantic fixtures. Generation requires Python >=3.11 and the repository's
pinned Rust toolchain (`rustfmt`); no Python generation dependencies are needed.

## Source and supported subset

`contracts.json` explicitly classifies each contract as `generated` or
`draft-validated`. Keys are unique CDDL file stems and matching top-level
fixture directory names. Every `spec/messages/*.cddl`, `spec/*.cddl` and
`spec/fixtures/*/` directory must be classified; stale entries, unknown modes,
duplicate keys/stems, empty fixture families and root-level JSON fixtures fail
`codegen/run --check`. New nested JSON fixtures are discovered recursively.

Common stays `generated`; policy, promotion and tickets are `draft-validated`
until M3 (not part of the v0 freeze). Drafts remain subject to real CDDL and
JSON Schema validation, semantic checks and golden CBOR round-trips in
`spec/tests/test_control.py`, run by conformance CI. Its registry coverage test
requires every draft family to have both validators registered. The Rust
schema-independent golden/profile test also continues to check draft goldens.
Promoting a contract to `generated` requires compiler support and cross-checks
in the same PR; changing the registry alone must not waive unsupported syntax.

Only CDDL files classified `generated` are compiled together, sorted by path, into
one validation IR and language records. The current CDDL subset includes named
rules/references, inclusive integer ranges, text regex/byte-size constraints,
literals, unions, closed records with optional fields, open text-key maps and
homogeneous `[* T]` / `[+ T]` arrays. Unsupported syntax, duplicate rules and
undefined references fail generation rather than silently widening a schema.
Extend the compiler with tests when a spec needs another CDDL construct.
This is a subset compiler, not a general CDDL implementation.

JSON Schema is an independent fixture cross-check, **not** the generation input.
Every fixture's rule must have an unambiguous `$defs` definition. Cross-file
JSON references resolve by the schemas' declared `$id` values.

## APIs

Rust (`coglet-types`): generated `Event`, `Action`, `Error`, `VersionOffer`,
`VersionSelection`, and untagged record unions `ActionAccept`, `ActionOutcome`.
Records expose `to_cbor` / `from_cbor`. The generic `encode(rule, &Value)` /
`decode(rule, bytes)` APIs also support scalar and union entry rules; typed
serde helpers `encode_typed` / `decode_typed` are available for unions.
Aliases keep CDDL names; heterogeneous/recursive values use `ciborium::Value`.
**Use the CBOR helpers at wire boundaries**: serde traits alone do not enforce
CDDL constraints or deterministic wire encoding.

Python (`coglet_surface.generated`): generated dataclasses inherit
`Message.to_cbor` / `from_cbor`. Record-union branches have numeric suffixes
(e.g. `ActionAccept1`, `ActionAccept2`) and a union alias `ActionAccept`.
Nested closed records decode into dataclasses; open payloads remain ordinary
Python dictionaries/lists/scalars. `coglet_surface.codec.encode(rule, value)` /
`decode(rule, raw)` handle every entry rule and return ordinary values.
Dataclass annotations alone are not validation; constraints are enforced at
encode/decode. Optional record fields use `None` to mean absent (current optional
fields cannot explicitly contain null).

Go (`sdk/surface-go/types`): generated structs, pointer optional fields, and
interfaces with numbered structs for record unions. Records expose `ToCBOR`
and `Decode<Type>` functions. Generic `Encode` / `Decode` support every rule.
Native strict CBOR and IR validation live in `sdk/surface-go/codec`.

Java 21 (`sdk/surface-java`): `Generated` contains records, sealed interfaces
for record unions, and `Presence<T>` for optional fields (null means absent).
Records expose `toCbor` / `fromCbor`; `Codec.encode` / `decode` handle every rule.
Integers use `BigInteger`; Jackson reads JSON schemas/fixtures, never CBOR.
Both emitters escape keywords and reject ambiguous sanitized identifiers.
See each SDK README for the typed API and presence semantics. Go/Java currently
consume the original (v1) IR and fail generation on new operations or additive
constraints, including bounded/keyed maps, bounded arrays and open-record rest.
Compiler IR extensions must update the native codecs and this guard together;
unsupported constraints must never be silently discarded.

All four runtimes enforce the `spec/conventions.md` CBOR profile before shape
validation: no bytes/tags/floats, no duplicate/non-text keys, no indefinite or
non-shortest forms, no trailing data, and exact UTF-8/integer bounds. Map keys
are ordered by their encoded bytes, recursively. Decoders/encoders bound CBOR
nesting to 64; schema expression traversal is bounded separately to 128.
Transport framing, temporal action lifecycle, negotiation and authentication
are deliberately outside this codec.

## Tests

```sh
python3 -m unittest discover -s codegen -v
codegen/run --check
cargo test -p coglet-types --locked -- --nocapture
uv run --project sdk/surface-python --locked pytest sdk/surface-python/tests -s
uv run --python 3.12 --with-requirements spec/tests/requirements.txt python codegen/cross_check.py
(cd sdk/surface-go && go test -v ./...)
(cd sdk/surface-java && mvn --batch-mode test) # JDK 21
```

Rust and Python generated-schema fixture tests recursively discover JSON files
under the registry's `generated` fixture families. Go and Java fixture selection
must use the same registry before draft families enter their native tests.
Fixture files currently contain arrays of semantic cases (`name`, `type`,
`valid`, `value`, and `cbor_hex` for valid cases) or wire-negative cases
(`name`, `cbor_hex`). New files within classified families are picked up without
updating the registry; new families require explicit classification.
Nonconforming fixture formats fail rather than being silently skipped.
At introduction: **33 valid golden**, **43 semantic-invalid**, and **17
wire-invalid** fixtures per language. These are schema/profile tests, not a
claim of protocol implementation conformance.

## Remaining targets

The Swift (`sdk/surface-swift`) emitter and its native runtime/fixture tests are
a follow-up. It must consume the same CDDL IR and run the same recursively
collected fixture corpus; no placeholder code for these targets is claimed as
implemented here.
