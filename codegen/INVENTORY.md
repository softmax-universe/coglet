# Frozen-spec construct inventory

Inputs inventoried directly from Git (all `spec/messages/*.cddl` plus root
`spec/*.cddl`, including drafts). Counts below are source rules, not aliases.

| Snapshot | Exact head | Files / source rules |
|---|---|---|
| main | `20ea0867c92b7cacef71dff6c5627787243462f9` | common 23 |
| ABI PR8 | `8978cdb00438d18df17d3c3100a804534bac8bc0` | common 23, abi 20 |
| surface PR9 | `0d3a28a7babf3aec844fc7c57f538494e9038f5c` | common 23, surface 15, speech 16, expression 4 |
| hub PR13 | `34fdd14e1038dc894f4eb4edfeb09079c1722b31` | common 23, hub 111 |
| policy PR19 | `834ac27bf09ce1a5b0adbd8a19c44fe530fe5cbb` | common 23, policy 8, promotion 20, tickets 31 |

## Complete syntax categories found

- Semicolon comments/whitespace; named `rule = type` declarations; rule aliases
  and cross-file references; bare and quoted text member names; parenthesized
  type expressions; scalar entry rules and unions as entry rules.
- `tstr`, `bool`, `null`, `nil`; true/false, integer and escaped JSON text
  literals; inclusive `..` integer ranges including uint64 maximum and negative
  signed-profile bounds; `/` type choices and nested combinations.
- `{}` records with required and `?` optional fields, inline records, record
  choices; `* tstr => T` maps, including recursive profile values; typed map
  keys `{* (capability-name) => T}`; field groups `(fields)` and inline `//`
  alternatives inside a map; named groups used as field splices.
- `[* T]`, `[+ T]` homogeneous arrays; bounded zero-to-max arrays `[* 17 T]`,
  `[* 64 T]`, `[* 256 T]`; bounded empty map `{* 0 tstr => profile-value}`.
- `.regexp` full-match text constraints; `.size (lo..hi)` UTF-8 byte lengths;
  `.and` intersections, notably the empty type `nil .and false`, both on rest
  map entries and optional sibling fields in alternatives.
- Portable regex constructs present: start/end anchors, literal/escaped text,
  character classes and ranges (including negation and `[\s\S]`), grouping,
  alternation, `.`, `?`, `*`, `+`, exact/ranged `{n}`/`{lo,hi}` repetitions.
  The large cases are surface/speech Unicode text up to 16,384 scalars and hub
  lowercase hex pairs repeated up to 32,768 times. Size is bytes; regexp
  character repetition is Unicode scalars, not bytes. No lookaround,
  backreferences, Unicode property escapes or engine-specific groups found.

## Per-file controls and distinguishing constructs

| File | Controls | Additional distinguishing constructs |
|---|---|---|
| common.cddl | 8 regexp, 1 size | recursive map/value/array union; `+` nonempty versions; full integer profile |
| abi.cddl | 3 regexp, 5 size | four `//` separators/five map-group choices; nullable refs; required record unions |
| surface.cddl | 2 regexp | typed/regexp map keys, parenthesized types, nested record/array references |
| speech.cddl | 5 regexp | Rust keyword `final`, field name `type` in related wrappers; record/literal unions |
| expression.cddl | none | inclusive ranges and text/record unions |
| hub.cddl | 7 regexp, 1 size | zero-entry maps; bounded arrays 17/64/256; scalar aliases; large hex regexp |
| policy.cddl | 1 size, 2 and | inline `//` group alternative, rest empty-type constraints |
| promotion.cddl | 1 regexp, 1 size, 4 and | named field groups, optional forbidden sibling fields, recursive maps |
| tickets.cddl | 2 regexp, 1 size, 2 and | named field groups, optional fields, nonempty arrays, empty-type constraints |

No `.default`, tuple, group socket, type socket or augmentation declaration is
currently used in these snapshots. They are nevertheless part of this card's
requested compiler support and have explicit unit/native tests: metadata-only
defaults, tuple backtracking, empty/open `$$g` sockets and `//=`, `$t` and `/=`.
Unsupported controls or constructs fail by name rather than widening a type.

## Namespace and fixture selection

All five complete snapshots parse and lower (including drafts for this
inventory): 46/86/116/268/138 qualified-and-unique-alias selectors respectively.
The compiler does not generate draft contracts merely because they parse.
The registry is the policy owner's exact implementation, adopted once with
main's common row; spec integrations add their frozen generated rows and retain
policy/promotion/tickets as draft-validated.

ABI `.fixture` files are protocol-owned temporal JSON, not codec case arrays.
Surface speech timelines/adapter config/framing carry `semantic-vectors-v1`
markers. Hub fetch/framing/git/signatures require those same markers in PR13;
the pinned Hub proof supplies exactly those four agreed metadata files as an
explicit integration delta (not a claim that PR13 already contains them).
Their owning suites remain responsible for semantic invariants. See README/IR
for fail-closed case classification and separate skip/semantic counters.

## Executed merged-snapshot codec proof

Each row includes common. Rust, Python, Go and Java independently produced the
same shape/wire counters and byte-identical supplied golden encodings. Every
row also passed `codegen/run --check` and the actual-CDDL/JSON-Schema cross-check.
Registry additions and generated outputs are explicit integration resolutions;
no draft is promoted and no unsupported construct is ignored.

| Snapshot | Shape-valid | Shape-invalid | Wire-invalid | Counted vector-file skips |
|---|---:|---:|---:|---:|
| Main | 33 | 43 | 17 | 0 |
| ABI | 48 | 62 | 17 | 0 |
| Surface | 76 | 109 | 17 | 3 |
| Hub + four marker metadata files | 143 | 303 | 17 | 4 |
| Policy (generated common only; drafts retained) | 33 | 43 | 17 | 0 |

Protocol-semantic negatives are shape-valid by definition and remain the spec
suite's responsibility; rejecting them in a shape codec would be incorrect.
These are codec/fixture proofs, not behavioral runtime-conformance claims.
