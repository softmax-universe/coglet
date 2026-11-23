# Message schemas

These are the v0 common, ABI, surface, speech, expression and hub CDDL/JSON Schema
contracts, not a reserved scaffold. `codegen/contracts.json` is the authoritative
family classification for generation and fixture crawling. Import common rules;
do not duplicate envelope, identity, version or error definitions.

A selected capability payload is validated using
[`capabilities/bindings.json`](../capabilities/bindings.json), not a root union.
[`capabilities/bindings.md`](../capabilities/bindings.md) defines both directions,
reserved names and gateway boundaries. Semantic traces under `spec/fixtures`
use `.fixture` or an explicit `semantic-vectors-v1` marker; their owning spec
tests are mandatory and are not interchangeable with shape-validation cases.

Run `pytest spec/tests`, `codegen/run --check`, the independent
`codegen/cross_check.py`, harness regression tests and `conformance/run` in the
pinned test environment. Schemas, goldens and executable case contracts are not
an implementation or native/device conformance claim. See the
[owned M1 qualification gates](../../docs/reviews/spec-v0-m1-gates.md).
