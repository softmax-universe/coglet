# Message schemas

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things.

These are the v0 common, ABI, adapter, speech, expression and router CDDL/JSON Schema
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
