# Dependency mirror operations

Design: [CI dependency mirror](design/ci-dependency-mirror.md). The initial Linux
consumers are Rust and conformance. Local contributor builds are unchanged.

## Normal runs and refreshes

Before any registry command, CI restores dependency-only release assets named
`<job>-<ecosystem>-<input-sha256>.{json,tar.gz}` from `dependency-mirror-v1`.
Each manifest records the source checkout SHA, compressed bytes and archive digest.
Keys include the platform, pinned language-tool versions, nested manifests/locks,
codegen dependency logic and mirror setup/workflow code. Whole hosted-image
versions are deliberately excluded: GitHub rolls multiple images out concurrently,
including within one preparation/proof run. Compiled `target/` outputs are not
restored.
Missing, corrupt or unavailable assets are logged. Cargo/rustup retain upstream
fallback on a normal miss. Proof restores with `--required` for Cargo and rustup:
a missing closure fails clearly instead of waiting on crates.io or rustup
distribution hosts. Python wheels and gems are convenience closures, not
whole-registry-outage proof. Local contributor builds remain unchanged.

`dependency-mirror` runs daily, on dependency/setup changes on main, or manually.
Read-only preparation runs the existing workflows' commands, fetches every Cargo
lock's target closure, and captures separate Cargo, rustup, Python-wheel and
gem-cache archives. Publication has no checkout and never executes downloaded
artifacts. Only its main-branch job receives ephemeral `contents: write`.
It adds new assets without clobbering existing ones; failed refresh leaves old
sets intact. Job summaries report bytes and digests. Retention is deliberately
manual; do not delete the last working dependency set during an outage.

Python wheels and gems cover these jobs only. The proof does not deny PyPI,
RubyGems, Ruby/Python bootstrap actions, GitHub checkout/action downloads or
WAMR's checksum-verified GitHub codeload archive. Go remains on setup-go caching;
there is no phase-one Go outage guarantee. No AWS account, OIDC role or long-lived
secret is required. A later AWS move must use the design's separated OIDC roles.

## Candidate proof before a mirror change merges

A mirror-code PR runs read-only `prepare-*` jobs and uploads `dependency-rust`
and `dependency-conformance` artifacts. It cannot publish shared
assets. The candidate-existence gate skips `proof-*` until the candidate is published;
that preparation-only success is not proof acceptance.
An authorized operator must:

1. Independently review the candidate source and preparation workflow. Verify the
   PR head and the Actions run's `head_sha`/PR association, both preparation
   results, and the uploaded artifact IDs. PR runs normally use GitHub's synthetic
   merge SHA; preserve that SHA as the preparation provenance, not as the PR head.
2. Download **only those two artifacts** into an empty directory using
   `gh run download RUN --name dependency-rust --dir DIR` (repeat the other name).
   Do not execute or extract the `.tar.gz` dependency archives. Inspect their JSON
   identities/digests and ensure they match the successful reviewed run.
3. Create a **draft** prerelease `dependency-candidate-PR_HEAD_SHA`, targeted at
   that exact head, with `gh release create ... --draft --prerelease --latest=false`.
   Upload only the named `.json`/`.tar.gz` assets with `gh release upload`; never use
   `--clobber` or the main mirror tag. Read back every asset's name, size and digest
   against the validated local files, then publish with
   `gh release edit TAG --draft=false --prerelease --latest=false`. Read back the
   published release and assets before rerunning CI. Record the source run URL,
   merge SHA, PR head and independent review in release notes.
   Publishing before uploading permanently locks an empty release when repository
   release immutability is enabled. Do not disable immutability, move its tag, or
   delete/recreate it: an immutable release's tag name cannot be reused. Preserve
   that failed candidate as non-acceptance evidence and use a new reviewed head's
   distinct candidate tag. Input-addressed archives may be reused only after
   verifying every compatibility input is unchanged; retain their original names,
   manifest source SHA, preparation run/artifact IDs and digests. The new head still
   requires its own actual proof, not the old head's preparation-only success.
4. Rerun the entire same run (`gh run rerun RUN`) so the candidate-existence gate
   is evaluated again; a failed-only rerun cannot reevaluate a successful gate.
   The reusable
   `proof-*` jobs restore on new hosted runners, disable Actions dependency caches,
   explicitly remove the pinned preinstalled Rust toolchain, then deny
   crates.io/index/static and rustup distribution hosts for IPv4 and IPv6. They log
   failing direct curl probes and set Cargo offline so later cargo invocations
   cannot contact those registries.
   The test/build commands are the normal workflows, not a smaller smoke suite.
5. Link the successful proof run on the PR. Independently review the final exact
   head; require all normal CI plus executed rust/conformance proof jobs to
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
