# Dependency mirror operations

Design: [CI dependency mirror](design/ci-dependency-mirror.md). The initial Linux
consumers are Java, Rust and conformance. Local contributor builds are unchanged.

## Normal runs and refreshes

Before any registry command, CI restores dependency-only release assets named
`<job>-<ecosystem>-<input-sha256>.{json,tar.gz}` from `dependency-mirror-v1`.
Each manifest records the source checkout SHA, compressed bytes and archive digest.
Keys include the platform/tool versions, nested manifests/locks, codegen dependency
logic and mirror setup/workflow code. Compiled `target/` outputs are not restored.
Missing, corrupt or unavailable assets are logged; ordinary CI uses upstream with
PR #59's existing Maven retries/cache. A simultaneous GitHub/upstream outage or
an unpublished dependency addition is not covered.

`dependency-mirror` runs daily, on dependency/setup changes on main, or manually.
Read-only preparation runs the existing workflows' commands, fetches every Cargo
lock's target closure, and captures separate Maven, Cargo, rustup, Python-wheel and
gem-cache archives. Publication has no checkout and never executes downloaded
artifacts. Only its main-branch job receives ephemeral `contents: write`.
It adds new assets without clobbering existing ones; failed refresh leaves old
sets intact. Job summaries report bytes and digests. Retention is deliberately
manual; do not delete the last working dependency set during an outage.

Python wheels and gems cover these jobs only. The proof does not deny PyPI,
RubyGems, Java/Ruby/Python bootstrap actions, GitHub checkout/action downloads or
WAMR's checksum-verified GitHub codeload archive. Go remains on setup-go caching;
there is no phase-one Go outage guarantee. No AWS account, OIDC role or long-lived
secret is required. A later AWS move must use the design's separated OIDC roles.

## Candidate proof before a mirror change merges

A mirror-code PR runs read-only `prepare-*` jobs and uploads `dependency-java`,
`dependency-rust` and `dependency-conformance` artifacts. It cannot publish shared
assets. The first `proof-*` attempts fail clearly until the candidate is published.
An authorized operator must:

1. Independently review the candidate source and preparation workflow. Verify the
   PR head and the Actions run's `head_sha`/PR association, all three preparation
   results, and the uploaded artifact IDs. PR runs normally use GitHub's synthetic
   merge SHA; preserve that SHA as the preparation provenance, not as the PR head.
2. Download **only those three artifacts** into an empty directory using
   `gh run download RUN --name dependency-java --dir DIR` (repeat the other names).
   Do not execute or extract the `.tar.gz` dependency archives. Inspect their JSON
   identities/digests and ensure they match the successful reviewed run.
3. Create a prerelease `dependency-candidate-PR_HEAD_SHA`, targeted at that exact
   head, with `--latest=false`. Upload only the named `.json`/`.tar.gz` assets with
   `gh release upload`; never use `--clobber` or the main mirror tag. Record the
   source run URL, merge SHA, PR head and independent review in release notes.
4. Rerun failed jobs of that same run (`gh run rerun RUN --failed`). The reusable
   `proof-*` jobs restore on new hosted runners, disable Actions dependency caches,
   explicitly remove the pinned preinstalled Rust toolchain, then deny Maven,
   crates.io/index/static and rustup distribution hosts for IPv4 and IPv6. They log
   failing direct curl probes, set Cargo offline and install a global offline
   Maven wrapper so Python-launched Maven subprocesses cannot contact Central.
   The test/build commands are the normal workflows, not a smaller smoke suite.
5. Link the successful proof run on the PR. Independently review the final exact
   head; require all normal CI plus executed java/rust/conformance proof jobs to
   pass before merging. A skipped proof or preparation-only pass is not evidence.

Any source update requires new head/provenance validation and a new candidate tag.
If only non-dependency source changes, input-addressed assets can be reused, but
never claim the old proof ran on the new head. Never add `pull_request_target` or
publication credentials to preparation to automate away this trust boundary.

## Local transport regressions

```sh
python3 -m unittest discover -s scripts/ci -v
actionlint
```

These cover transport roundtrip, digest-corruption atomicity, archive traversal,
escaping links/unknown roots, key invalidation and miss/fail-closed behavior. They
do not replace the hosted registry-denied proof.
