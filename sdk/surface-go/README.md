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

Run `go test -v ./...` from this directory. Tests recursively discover all
`spec/fixtures/**/*.json`, enforce minimum corpus counts (33 golden, 43 semantic
negatives, 17 wire negatives), and byte-identical golden encode/decode. Generate
with repository `codegen/run`; don't hand-edit `types/generated.go`.
