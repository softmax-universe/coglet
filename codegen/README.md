# CDDL code generation

Run `codegen/run` and commit its Rust, Python, Go and Swift outputs.
`codegen/run --check` is read-only and fails on stale/missing output.
Generation requires Python >=3.11 and the pinned Rust `rustfmt`.

## Explicit contract selection

`contracts.json` is the exhaustive generated/draft-validated registry, adopted
from the policy owner rather than introducing a second registry. Keys are CDDL
file stems and matching top-level fixture directories. `contracts.py` rejects
unclassified files/families, duplicate or stale entries, unknown modes, empty
families and root-level JSON fixtures. Nested fixture files are discovered.
Only `generated` contracts enter codecs; `draft-validated` contracts retain their
protocol-owned actual-CDDL/JSON-Schema/semantic tests. Draft exclusion is not a
claim of compiler or protocol conformance. Frozen v0 contracts must be generated.

## Compiler and IR

See [IR.md](IR.md) for the additive v2 node contract. The supported subset is:
- rules/references, type unions `/`, inclusive integer ranges, text/bool/null and
  integer/text/boolean literals;
- named/inline groups, `//` group alternatives, group sockets `$$g`/`//=`,
  type sockets and `/=` augmentation (groups lower away before emission);
- closed and open text-key maps, required/optional fields, typed/regexp map
  keys, occurrence bounds (`?`, `*`, `+`, `n*m`, including zero maximum);
- homogeneous bounded arrays and ordered tuples with optional/repeated entries;
- intersections `.and`, UTF-8 byte `.size`, portable full-match `.regexp`,
  metadata-only `.default` (never insert fields or alter wire bytes).

Unsupported controls, syntax, recursive/repeated group splices, undefined or
ambiguous imports and identifier collisions fail loudly. This is not a general
CDDL implementation. Every runtime rejects unknown operations; the shared native
boundary also rejects unknown metadata. New constraints need codec tests, not
just a broader guard. The Go native codec implements v2 in this change.

`parse_files({stem: source})` resolves references local-first, then `common`,
then a unique exporter. Every rule has a `file::rule` selector; unique rules also
have unqualified selectors. Emitters normalize identifiers and reject collisions
with generated types, variants, fields or runtime-owned methods. Reserved field
keywords are escaped with wire-name metadata. Runtime method/import collisions
are rejected where escaping would conflict with an existing public API.
Qualification is not new CDDL syntax.

Python class-body helpers use `_coglet_dataclasses`, a namespace forbidden to
schema fields. Rust reserves `Result` and serde derive names as well as its
container types; schema field casing is retained without a lint failure. Go
exports fields but keeps package/local helpers separate, and rejects fields
colliding with `ToValue`, `ToCBOR` or synthetic `Rest`. No wire key is renamed.
`test_identifiers.py` executes generated Python imports and typed roundtrips,
native Rust (including clippy), Go helper/keyword roundtrips, and
explicit rejection tests. Required and optional Python `field` precede another
field to exercise class-body evaluation, not merely syntax compilation.

## APIs and wire profile

- Rust: `coglet-types` records, untagged record unions, `to_cbor`/`from_cbor`,
  generic `encode`/`decode`, and typed serde helpers.
- Python: `coglet_surface.generated` dataclasses, `Message.to_cbor`/`from_cbor`,
  generic codec functions. Wire metadata preserves keyword/dashed field names.
- Go: generated structs and record-union interfaces, `ToCBOR`/`Decode<Type>`,
  generic `types.Encode`/`Decode`. Optional fields are pointers.

Use codec methods at wire boundaries: language types/serde alone do not enforce
CDDL constraints. Heterogeneous values use each runtime's generic value model.
Python uses None for absent optional fields (explicit nullable optionals need
separate presence modeling). All codecs enforce deterministic profile CBOR:
no bytes/tags/floats, duplicate/non-text keys, indefinite/non-shortest forms or
trailing data; map keys sorted by encoded bytes; nesting and recursion bounded.
Transport, authentication and temporal invariants belong to protocol suites.
Swift consumes the same documented IR.

### Swift (`CogletTypes` in `sdk/surface-swift`)

Generated public structs and indirect associated-value enums are Codable-free.
Union cases are `variant1`, `variant2`, etc. in CDDL order; inline types receive
`VariantN`/field-name suffixes. Records have public memberwise initializers and
`toCBOR()` / `fromCBOR(_:)`. Native scalar aliases use Swift types; intersections
use `CBORValue`, tuples use `[CBORValue]`, and open records/maps use
`[CBORMapEntry]`. Generic `CBORCodec.encode(rule:value:)` / `decode(rule:bytes:)`
supports every entry selector, including qualified `file::rule` selectors.

Map entries deliberately avoid Dictionary<String, ...>: Swift String equality
normalizes canonically equivalent Unicode text, whereas wire keys compare by
exact UTF-8 bytes. Optional fields use nil for absence and `.null` for explicit
profile null where permitted. Construction alone is not schema validation.

The native, dependency-free codec in `CBOR.swift` parses the small wire profile
directly, rejecting evidence that permissive libraries can normalize away:
non-shortest heads, tags, forbidden simple values and out-of-order keys. It
checks lengths against remaining input before allocation/integer conversion.
Wire nesting is bounded to 64 container edges; transparent schema hops per value
are separately bounded to 512 and tuple matching to 100,000 work steps. Generated
validation IR and types live in `Sources/CogletTypes/Generated/Types.swift`.

Fields are backtick-escaped, hyphens map to underscores and runtime helper names
receive trailing underscores; original wire keys are explicit string literals.
Type/field/helper/inline-name collisions and nonproductive recursive type cycles
fail generation. The shared native IR boundary rejects unknown constraints.
Synthetic keyword and expanded-IR tests compile and execute generated Swift.
Generated records/unions intentionally do not synthesize Equatable: Swift's
native text equality is not wire identity. Compare validated `toCBOR()` bytes or
their `CBORValue` representations when exact protocol equality is needed.

`swift test --package-path sdk/surface-swift` recursively discovers fixtures,
selects generated families from the sole `contracts.json` registry and rejects
unknown/missing classifications. Draft exclusions, marked vectors and protocol
semantic cases are counted separately, never reported as codec passes. The
required `swift-codegen` conformance prerequisite runs these tests and emitter
regressions on `macos-latest`; Linux Swift 6.0.3 supports local testing. The
freshness regression mutates every output, including Swift, in an isolated copy.

## Fixture formats

An unmarked JSON file is a nonempty codec case array. Each entry is either:
- a shape case: `name`, `type`, boolean `valid`, `value`, and `cbor_hex` for
  positive golden cases; invalid values must fail encoding and typed decoding;
- a wire negative: `name`, `cbor_hex`, no type/valid; profile decoding must fail.

`semantic: true` marks a protocol-semantic negative: its shape must still pass,
including when `valid: false`; its semantic rejection is tested by the owning
protocol suite and counted separately. This never excuses an unknown type or
invalid shape. Every unmarked malformed case file is an error.

Protocol-owned non-case vectors MUST have an adjacent `<full-filename>.format`,
e.g. `signing.json.format`, containing exactly `semantic-vectors-v1` with at most
one final newline. All fixture consumers explicitly count these files skipped;
they still require valid JSON. Unknown markers fail. The protocol suites still
execute the vectors; codec skips are never protocol passes. No format guessing
or catch-and-skip parse errors. Draft-family exclusions are separate counts.

Cross-check uses actual CDDL and independent JSON Schema, not generated IR as an
oracle. `spec/tests/reference_cddl.py` always executes the Ruby CDDL backend
pinned by `spec/tests/Gemfile.lock`; it never switches validators according to
an expected fixture label. Install it with `BUNDLE_GEMFILE=spec/tests/Gemfile
bundle install` (set `BUNDLE_PATH` explicitly when using a non-default install).
Schema/tool failures are fatal `ReferenceError`, distinct from an instance
`ValidationError`. See `spec/tests/REFERENCE-CDDL.md` for API and regressions.

## Tests

```sh
python3 -m unittest discover -s codegen -v
codegen/run --check
cargo test -p coglet-types --locked -- --nocapture
swift test --package-path sdk/surface-swift
uv run --project sdk/surface-python --locked pytest sdk/surface-python/tests -s
uv run --python 3.12 --with-requirements spec/tests/requirements.txt python codegen/cross_check.py
(cd sdk/surface-go && go test -v ./...)
```

Compiler regressions cover groups/sockets/choices, controls, namespaces,
tuples/bounds, typed map keys, fixtures and collisions. Native tests check
at-bound accept / past-bound reject on encode and decode, including tuple
backtracking, empty intersections and metadata-only defaults. Main's common
corpus has 33 goldens, 43 shape negatives and 17 wire negatives per language;
this is schema/profile evidence, not protocol implementation conformance.
