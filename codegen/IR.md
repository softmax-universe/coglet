# Compiler IR and fixture formats (version 2, additive)

The compiler owns this IR. Language emitters must reject unknown operations.
Rule dictionaries contain `file-stem::rule` keys and, only when globally unique,
an unqualified copy of the same rule. References resolve local first, then
`common`, then a single unique exporter; ambiguous imports fail compilation.
`parse(text)` remains the single-namespace API; `parse_files({stem: text})` is
the multi-file API. Qualification is not CDDL syntax. Never concatenate files
with duplicate entry rules such as `wire`.

## Runtime nodes

Existing: `ref {name}`, `literal {value}`, `primitive {name: tstr|bool}`,
`range {min,max}`, `union {items}`, `regexp {pattern}`, `size {min,max}`,
`record {fields:[{name,optional,type}]}`, `array {item,min,max?}`,
`map {item,key?,min?,max?}`. All bounds are inclusive. An omitted max is unbounded.
`size` is UTF-8 text byte length (the currently inventoried use).

Added:
- `intersection {items}`: every child must validate the same value. It is not
  map field merging; groups perform field merging before runtime.
- `default {item,value}`: validate item. Value is default metadata only; do NOT
  insert missing fields, change wire bytes, or broaden acceptance.
- `tuple {items:[{type,min,max?}]}`: ordered array entries with inclusive
  occurrences. Match the whole array. Optional/repeated entries may require
  trying multiple positions, not a greedy match.
- `record.rest? {item,key?,min,max?}`: applies to otherwise-unclaimed keys only.
  No rest means closed map. `nil .and false` lowers to intersection of two
  incompatible literals, so it is the empty type, NOT the literal false.

Named groups, inline groups, group choices `//`, group sockets `$$name` with
`//=`, and type augmentation `/=` are compile-time constructs. Groups splice
and distribute into records/unions/tuples; no group node reaches emitters.
Recursive group expansion and repeated/optional group splices fail explicitly.
Only text map keys are supported by the wire profile. Supported regexp syntax
is the common ASCII subset, without lookaround, engine-specific groups,
backreferences or shorthand character classes. Unsupported controls name the
operator in the error. Type/field identifier collisions fail before emission.

## Fixture discovery contract

A JSON file in spec/fixtures is a case array unless an adjacent file named
`<full-filename>.format` contains exactly `semantic-vectors-v1` (optional final
newline). This marker declares a protocol-owned semantic/vector fixture, not a
codec case array. Every consumer reports the number of marked files skipped;
protocol suites still execute their own vectors. Unknown marker values fail.
Malformed case arrays fail; never skip by catching parse errors or guessing
from object structure. The marker is test metadata, not a wire-spec change.

Case arrays have named shape cases (`type`, `valid`, `value`, optional
`cbor_hex`) or wire negatives (`name`, `cbor_hex`). A shape-negative must fail
validation and decoding. A valid shape with a golden must round-trip exactly.
The optional boolean `semantic: true` marks a protocol-semantic negative:
its SHAPE must validate even when `valid: false`; its semantic outcome belongs
to the protocol suite, and consumers count semantic cases separately. A
semantic marker never authorizes an unknown type or malformed shape. This
prevents conflating hash/signature/temporal invariants with CDDL constraints.
ABI trace files without type selectors use semantic-vectors-v1 markers and
remain exercised by spec/tests/test_abi.py. Hub signatures/git/framing/fetch
files use the same marker and remain exercised by spec/tests/test_hub.py.
