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

## Selected-profile workflow generation

The selected-profile peer step was introduced by PR171 at immutable boundary
`ebad856289393a2a770478a038fe12296bc29385`. Applicability is computed from
that commit's ancestry in the **actual selected SHA**, after the unchanged source
selector runs. This is independent of the older transform/exact-head boundary.
A head between the two boundaries still runs both architectures and the Rust
aggregate on its exact head; only the newer selected-profile step and its
artifact are not applicable. The summary records the boundary and explicitly
says that selected-profile qualification was **not executed or qualified**. A
green check on such a head is not evidence of selected-profile qualification.
Its modern merge-group and subsequent main execution must run that qualification.
No checkout, manifest identity or publication scope is substituted or relabeled.

For sources descending from the boundary, the source job first requires regular
Git blobs for the bridge, its daemon test, the selected-profile registry and the
expect-close test. Deletions, directories, symlinks, malformed or unavailable
commit objects and shallow ancestry fail closed; they never switch applicability
off or select another source. The profile step remains required for modern
sources, and its artifact upload still runs on failure with missing-file errors.
The workflow output is compared explicitly with `'true'`, never as a truthy
string. This compatibility classification is not a waiver for modern trees.

PR191's exact head `f6393be19f20e1f8b9eb069f247e94ad4a46f78b` is between
these boundaries. Run45710926076 applied PR171's newer step to it and failed
opening an absent bridge test, before that qualification executed. The repair
preserves that old head instead of backporting unrelated runtime code or treating
missing files as permission to skip a modern test.

## R3 publication/runtime workflow generation

PR206 introduced the newer required R3 publication and runtime closure at
immutable boundary `e88fc139b3fd2b43b84136f2e7a33fb8c0f33a54`. As with the
selected-profile step, applicability is computed from that commit's ancestry in
the **actual selected SHA** after the unchanged source selector runs, never from
file existence: an older exact source whose tree coincidentally contains the files
is still classified false. The two generations are independent; a head that has
the selected-profile step but predates PR206 does not get the R3 steps.

For sources descending from the boundary, the source job first requires complete
(non-shallow) ancestry and regular Git blobs for `check_r3.py`,
`test_check_r3.py`, the hub `Cargo.toml`, `tests/build/qualified-ci.json` and the
`r3.rs`, `r3_live.rs`, `r3_guest.rs`, `r3_runtime.rs` build tests. Deletions,
directories, symlinks, unknown commit objects and shallow history fail closed.

The source job outputs `r3-required` and `r3-boundary`. The publication and ARM
jobs stay unconditional (the strict all-needs success checks are unchanged), and
each begins by reporting the actual SHA and boundary. For an older source it
states R3 publication/runtime qualification was **NOT EXECUTED/QUALIFIED**. Only
the R3 commands, artifact downloads and uploads, and the Rust aggregate's R3
runtime-pair download and `check_r3.py --aggregate` are step-gated on
`r3-required == 'true'`. All checkouts remain the selected SHA, and the native
pair and selected-profile steps are untouched.

**An older source gets no publication/runtime qualification from a green check.**
Its actual modern merge-group (or subsequent main) execution must run R3.

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
