# Go surface message types

`types` contains generated message structs and record-union interfaces. `codec`
implements the native strict deterministic CBOR profile and shared CDDL IR
validator. No external CBOR dependency is used.

```go
import "github.com/softmax-universe/coglet/sdk/surface-go/types"

offer := types.VersionOffer{Versions: []string{"0.1"}}
raw, err := offer.ToCBOR()
decoded, err := types.DecodeVersionOffer(raw)
```

Check errors on every encode/decode. Generic `types.Encode(rule, value)` /
`types.Decode(rule, raw)` support every CDDL entry rule. Generic integers decode
as `uint64` or negative `int64`; lists and maps are `[]any` / `map[string]any`.
`[]byte` / `[]uint8` and byte arrays are rejected as forbidden byte strings;
use `[]uint64` or `[]any` for arrays of integers. Typed nil pointers encode as
null and are rejected when the selected schema requires a record.
Record-only unions expose numbered structs and `DecodeActionAccept` (for
example) selects a validated branch. Optional struct fields use pointers; nil
means absent. An optional `any` pointer can explicitly represent null.

The codec rejects bytes, floats, tags, invalid UTF-8, non-text/duplicate/unsorted
map keys, non-shortest or indefinite encodings, trailing data, and integers
outside `-2^63..2^64-1`. CBOR nesting is limited to 64 and schema traversal to
128. These are message codecs, not transport or temporal protocol conformance.

The validator consumes additive IR v2: intersections validate every branch,
`default` values are metadata only, tuples backtrack over optional/repeated
entries, and array/map/rest occurrence and map-key constraints apply on both
encode and decode. Open records expose a typed `Rest` map for unclaimed keys.
Generic entry selectors accept `file::rule`; uniquely exported qualified and
unqualified names share a Go type alias, preserving native type identity.

Run `go test -v ./...` from this directory. Tests select generated fixture
families using `codegen/contracts.json`; draft-validated families remain owned
by their protocol suites. Adjacent `.json.format` markers explicitly declare
`semantic-vectors-v1` files and every run reports the skipped count. Unknown
markers and malformed case arrays fail. `semantic: true` cases must pass shape
validation even when protocol semantics mark them invalid. Tests retain minimum
corpus counts (33 valid shapes, 43 shape negatives, 17 wire negatives) and check
byte-identical goldens when supplied. Generate with repository `codegen/run`;
don't hand-edit `types/generated.go`.
