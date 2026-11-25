# Required Rust source and transform publication provenance

The required `rust` workflow tests both native architectures and their paired
aggregate. Source selection is shared by all three jobs and reported in the
`source` job summary. Neither architecture is optional; selector failure or
missing/skipped/failed native execution fails the required aggregate.

## Two honest scopes

- **exact-head**: PR heads descending from the reviewed exact-head qualification
  contract `87341a29b3716d3e147df041cfdf4df0507a74d8` (PR159) run against that
  unchanged head. Push and merge-group events run against their event SHA.
- **merge-integration-only**: older PR heads, including branches created before
  transform sources existed, run against the event's immutable synthetic merge
  SHA. The selector verifies that this is a two-parent merge whose second parent
  is the event's exact PR head, and that the selected tree descends from the
  qualification contract. It never substitutes today's mutable merge ref.

This compatibility policy applies to pre-contract history, not only one PR.
Ancestry is intentional: inspecting file presence would let a modern PR delete
qualification files and silently switch to a different source. Modern deletions
instead fail when their own exact source is tested. Invalid/missing objects and
Git errors fail selection, never imply legacy compatibility. Merge-queue events
still execute both native legs and the aggregate on the actual merged result.
There is no routine branch catch-up requirement.

**Integration-only success is not exact-head transform publication qualification.**
`check.py` continues recording `git rev-parse HEAD` as `actual_checkout` and
pairing against the real selected checkout. No manifest fields, required-job
names, artifact digests or publication verifier checks are changed. GitHub's PR
run head and a legacy synthetic merge differ; the authenticated publication
bridge must reject such artifacts as qualification of that PR head. Do not relabel
them. Consumers needing head qualification must use the authenticated bridge,
not infer it from a green required Rust check. The merge-group and subsequent
main runs qualify their own actual source identities, not the old PR head.

## Regression

```
python3 scripts/ci/test_rust_source.py -v
python3 crates/coglet-transform/tests/check_runner.py
```

The first command exercises real disposable Git histories for legacy/current
heads, deletion, unknown objects, wrong merge binding, and merge-group/push
selection; it also parses the actual workflow and asserts shared source wiring,
unconditional architecture execution and aggregate failure propagation. Fixtures
are not execution evidence. The unchanged second command retains the native
report and pairing refusal controls, including wrong-head/missing/ignored tests.

## Failure analysis

PR151 head `8f1b59adfa6915392ffa2a4e68c85624f225a0ef` had no transform
sources. Run45565662717 nevertheless applied the newer workflow to that old
checkout, so both native jobs failed opening `tests/check_runner.py` before any
tests executed. PR159 correctly fixed synthetic-merge artifacts being presented
as exact-head publication evidence, but applied that checkout policy to required
integration coverage too. Its tests covered provenance rejection and a modern
transform-bearing head, not workflow/source-generation compatibility. The shared
selector separates the two scopes without weakening publication identity or
removing any native execution. Reverting this repair restores the incompatibility,
not a check waiver.
