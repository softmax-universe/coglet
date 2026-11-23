# CDDL code generation

Run `codegen/run` from any directory; commit its Rust, Python and Swift outputs.
`codegen/run --check` is read-only and exits nonzero if any output is stale
or missing. The conformance CI job enforces this, exercises the generator and
cross-checks the generated validator against actual CDDL and JSON Schema on
all semantic fixtures. Generation requires Python >=3.11 and the repository's
pinned Rust toolchain (`rustfmt`); no Python generation dependencies are needed.

## Source and supported subset

All `spec/messages/*.cddl` files are compiled together, sorted by path, into
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

Swift (`CogletTypes` product in `sdk/surface-swift`): generated public structs
and indirect enums with associated values, without Codable. Union cases are
`variant1`, `variant2`, etc. in CDDL order; inline record/union types receive
`VariantN` / field-name suffixes. For example, `ActionAccept.variant1` carries
an `ActionAcceptVariant1` struct. Records have public memberwise initializers;
`toCBOR()` / `fromCBOR(_:)` validate at wire boundaries. Scalar aliases retain
native types; use `CBORCodec.encode(rule:value:)` / `decode(rule:bytes:)` for
arbitrary CDDL entry rules. `CBORValue` represents the exact profile integer
domains (UInt64 and strictly negative Int64), text, bool, null, arrays and maps.
`ProfileValue` and other unions are generated enums, including recursive ones.

Swift maps are `[CBORMapEntry]`, **not** `[String: CBORValue]`: Swift String
equality treats canonically equivalent Unicode text as equal, while the wire
profile preserves the original UTF-8. Entry keys compare bytewise and duplicate
keys are rejected during encoding. This also avoids normalizing text on decode.
Optional properties use Swift `nil` for absent; `.null` is an explicit profile
null where allowed. Ordinary construction is not schema validation.

The Swift codec is native and dependency-free. This is intentional: the small
wire profile does not need a general-purpose CBOR library, and parsing directly
preserves evidence of forbidden tags, non-shortest heads and simple-value forms
that permissive decoders often normalize away. Lengths are checked against the
remaining input before integer conversion/allocation. Runtime code is handwritten
in `CBOR.swift`; generated types and validation IR live in `Generated/Types.swift`.

Swift fields are backtick-escaped (including contextual keywords); hyphens map
to underscores and helper/special names get a trailing underscore. Wire keys
are always emitted as original string literals, never inferred from properties.
Type/field/helper/inline-type naming collisions and non-identifier field names
fail generation instead of silently changing the wire shape. Compiler extension
tests compile and execute a synthetic keyword-heavy schema with Swift itself.
Swift-specific naming validation lives in `codegen/swift.py`, not the parser.

All three runtimes enforce the `spec/conventions.md` CBOR profile before shape
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
swift test --package-path sdk/surface-swift
uv run --project sdk/surface-python --locked pytest sdk/surface-python/tests -s
uv run --python 3.12 --with-requirements spec/tests/requirements.txt python codegen/cross_check.py
```

Rust, Python and Swift fixture tests recursively discover `spec/fixtures/**/*.json`.
Fixture files currently contain arrays of semantic cases (`name`, `type`,
`valid`, `value`, and `cbor_hex` for valid cases) or wire-negative cases
(`name`, `cbor_hex`). New files are picked up without updating a manifest.
Nonconforming fixture formats fail rather than being silently skipped.
At introduction: **33 valid golden**, **43 semantic-invalid**, and **17
wire-invalid** fixtures per language. These are schema/profile tests, not a
claim of protocol implementation conformance.

The conformance workflow runs Swift fixture tests and emitter compilation tests
on `macos-latest`. Its existing `conformance` job depends on `swift-codegen`, so
that gate cannot pass when Swift fails. The read-only freshness test also mutates
Swift output in an isolated temporary repository and verifies stale detection.
Linux Swift 6.0.3 is supported for local codec/emitter tests; macOS CI remains
the native platform gate. `swiftc` is needed for the emitter compilation test
(it is explicitly skipped when absent in the Linux Python-only job).

## Remaining targets

Go (`sdk/surface-go`) and Java (`sdk/surface-java`) emitters and their native runtime/fixture tests are
follow-ups. They must consume the same CDDL IR and run the same recursively
collected fixture corpus; no placeholder code for these targets is claimed as
implemented here.
