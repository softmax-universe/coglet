# Independent reference CDDL validation

This is test infrastructure, not a protocol/ABI change. Common rules, JSON
Schemas, generated outputs and the exhaustive contract registry are unchanged.

## Setup and API

Use Python 3.12 and the pinned Python requirements. Install Ruby 3.2 and Bundler
2.5.23, then install the existing surface lane's exact Ruby lock:

```sh
BUNDLE_GEMFILE=spec/tests/Gemfile bundle install
# Optional isolated install; use the same absolute BUNDLE_PATH on every run:
BUNDLE_GEMFILE=spec/tests/Gemfile BUNDLE_PATH="$PWD/.bundle-gems" bundle install
python3 -m unittest discover -s spec/tests -v
python3 codegen/cross_check.py
```

For spec-owner tests in `spec/tests`, replace
`from pycddl import Schema, ValidationError` with
`from reference_cddl import Schema, ValidationError`. Call
`Schema(actual_cddl_document).validate_cbor(actual_bytes)` as before. Schema
construction compiles the real document. Validation returns `None` on a match;
`ValidationError` means a well-formed instance does not match. `ReferenceError`
means schema compilation, CBOR decoding, startup, transport or timeout failed:
**do not catch it as an expected negative**. Other entrypoints must add
`spec/tests` to their import path, as `codegen/cross_check.py` does.

The independent implementation is Ruby `cddl==0.12.14` with all transitive
versions pinned in `Gemfile.lock`. CBOR is decoded directly by its pinned
`cbor-diag` dependency; JSON transports schema text and CBOR hex, not values.
Byte strings therefore never become text and integer precision is preserved.
This does not replace the separate deterministic/profile wire checks.

A serialized, on-demand subprocess amortizes startup. Its parser cache is
bounded to 32 schema documents; no validation verdicts or expected fixture
labels are cached or supplied. Every invocation performs real validation.
Tool failures are fatal, with a 30-second response deadline. The worker is
closed/reaped at interpreter exit; tests exercise reuse, rejection/recovery,
shutdown/restart and memory-limited repeated negatives.

## Why this backend

Minimal failure on fresh `pycddl==0.6.4` (Rust `cddl==0.9.5`):

```python
Schema('root = {kind: "x" / "y"}').validate_cbor(cbor2.dumps({'kind': [1]}))
# incorrectly accepts; likewise root = {kind: "x"} / {kind: "y"}
```

The same bytes fail a single-literal schema and the independent Ruby backend;
valid `{'kind': 'x'}` passes. Rust `validator/cbor.rs` sets
`is_multi_type_choice` at 682–684, dispatches scalar values on arrays through
`validate_array_items` at 3569–3572, and records no error at 502–504 when there
is no group index and the choice flag is true. Per-path CDDL intersections
cannot establish a correct shared oracle.

The detection gap was top-level-only substitutions. The previous cross-check's
expected-result-dependent optional Ruby fallback also made backend selection
fixture-label-dependent. The shared backend now always runs independently;
CDDL, JSON Schema and generated results remain separate agreement checks.
`pycddl` remains pinned only for legacy branches pending their import migration;
it is not used by the shared reference or generated cross-check.

## Pinned Ruby adapter corrections

The adapter subclasses the independently implemented validator, not generated
Coglet logic, with two narrow corrections covered by actual RED/GREEN tests:

* `cddl.rb:79–80,843–860,863–866` accumulates `@last_message` across requests.
  Repeated recursive negatives exhausted memory. Reset only this diagnostic
  buffer before validation. The stress test previously exhausted 512MiB and now
  completes 600 verdicts under that address-space limit.
* `map_check` at 975–999 consumes a key before checking its occurrence upper
  bound, so `*0`/`0*0` can accept entries. Remove zero-occurrence members from
  the consumption sequence, leaving keys for subsequent members and ordinary
  closed-map rejection. Do not forbid keys globally: a later member may
  legitimately consume them. Tests cover typed/literal keys and that control.

No imported rule is rewritten, narrowed or widened. A future gem upgrade must
remove these adapters only with the same regressions passing.

CDDL occurrence bounds have no internal whitespace (RFC 8610 Appendix B:
`occur = [uint] "*" [uint]`). For example, `*0` is a zero bound, while `* 0`
starts an unbounded occurrence followed by an integer. Spec owners must correct
invalid spaced bounds; the backend does not silently repair source syntax.

## Cross-spec lexical guards

`test_schema_lint.SchemaLintTests.test_repository_schema_lexical_guards` runs in
normal CI. It scans every `spec/**/*.json` pattern using the actual
Python/jsonschema search semantics (not a replacement full-match engine).
Positive witnesses come from all schema/fixture strings plus 32 reproducibly
seeded `rstr==3.2.2` generations per pattern. A matched witness plus appended LF
must reject; inability to produce a positive witness is a coverage error. This
is deterministic witness testing, not a proof for every possible string.
`python3 spec/tests/schema_lint.py --output lint.json` reports concrete witnesses
and hit counts per file; each spec owner fixes its own source text.

The same guard rejects spaced occurrence bounds outside CDDL literals/comments
in `spec/**/*.cddl`, and in generated Rust/Python/Swift/Go/Java sources and
`schema.json` IR outputs. The generated source scan retains target-language
strings, which may carry CDDL text. Tests demonstrate unsafe `$` RED versus
absolute-end GREEN, zero bound spacing, lexical exclusions, generated-source
coverage and fail-closed missing witnesses.

## Recursive audit

`test_reference_cddl.py` includes the original repro, 100 scalar/container
negatives with 20 nesting controls, legitimate array choices and mixed nested
profile maps, byte/text distinction, malformed-tool errors and bounded-memory
stress. The common recursive audit runs in the normal spec test suite.
Additional CDDL imports are explicit (`--include promotion` for tickets); they
are appended after the owning entry rule and before common, without rewriting
any document. A parser/tool failure aborts rather than yielding a negative pass.
The draft promotion snapshot at 9c4ca108 uses direct `.and []`, which the pinned
Ruby grammar does not parse (its old type2 excludes bare arrays/maps). That
snapshot audit is a reported tool failure, not zero-mismatch evidence; no draft
rule is rewritten here. Frozen common/ABI/surface/hub audits do not need it.

```sh
python3 spec/tests/cddl_differential.py --family common
# Validate a spec-owner snapshot, selected rules AND its actual public root:
python3 spec/tests/cddl_differential.py --spec /path/to/snapshot/spec --family abi
python3 spec/tests/cddl_differential.py --spec /path/to/snapshot/spec --family surface
python3 spec/tests/cddl_differential.py --spec /path/to/snapshot/spec --family hub \
  --root hub-frame --root hub-request --root hub-result --output hub-audit.json
```

Every map value and array element at every depth is substituted with integer,
text, bytes, boolean, null, empty/nonempty/nested arrays and maps. Every map gets
an unknown key. Unmodified controls and all existing shape negatives remain;
all comparisons use the independent CDDL backend and real JSON Schema roots and
selected definitions, never generated validators. Reports count accepted
controls, wire cases (owned by wire tests), and explicitly marked non-codec
vector files (owned by protocol tests). Unknown classifications, malformed JSON
and tool errors fail the audit. Schema-shape validation is not runtime or
engine conformance evidence.
