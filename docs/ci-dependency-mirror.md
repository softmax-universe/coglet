# Dependency mirror operations

Design: [CI dependency mirror](design/ci-dependency-mirror.md). The initial Linux
consumers are Java, Rust and conformance. Local contributor builds are unchanged.

## Normal runs and refreshes

Before any registry command, CI restores dependency-only release assets named
`<job>-<ecosystem>-<input-sha256>.{json,tar.gz}` from `dependency-mirror-v1`.
Each manifest records the source checkout SHA, compressed bytes and archive digest.
Keys include the platform, pinned language-tool versions, actual Maven executable
version, nested manifests/locks, codegen dependency logic and mirror setup/workflow
code. Whole hosted-image versions are deliberately excluded: GitHub rolls multiple
images out concurrently, including within one preparation/proof run. Compiled
`target/` outputs are not restored.
Missing, corrupt or unavailable assets are logged. Cargo/rustup retain upstream
fallback. **Maven normal/proof jobs never fall back to Central:** after mirror and
Actions cache restore, `resolve_maven.py` invokes the fully versioned
`org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline` with `--offline`.
It does not perform plugin-prefix/latest-version lookup or force metadata refresh
with `-U`. Successful normal/proof resolution exports `MAVEN_ARGS=--offline` for
subsequent Maven invocations (including Python subprocesses). Preparation leaves
later full tests online until capture, including dynamically loaded Surefire
providers that go-offline alone does not fetch. A missing closure fails clearly
instead of waiting on Central. Local contributor builds remain unchanged.

Only explicit `bundle-mode: prepare` may fill a cold Maven cache online, after
trying offline first. On a preparation miss it first seeds from the exact original
Java Maven archive in reviewed candidate `9b5f54198673ff089905d82db874ace4a27bd101`,
verified against hardcoded SHA-256 `5957675c303a908498be1b2c9e88475f691806dc043c7046e087fe87ef52a061`.
This read-only GitHub bootstrap is a partial cache seed: original asset identity
and provenance remain unchanged, full tests still run, and capture emits a genuinely
new input-addressed closure. Normal/proof cannot use this seed as a substitute for
current input keys. This lets preparation fill/save a cold cache even while Central
returns 429 and the first shared publication is unavailable. The candidate must
be retained until a reviewed replacement seed is committed.
A shared monotonic 165-second budget bounds resolution and seed download together;
both Java and conformance also impose a native three-minute step timeout. The
empty-cache regression invokes real Maven offline and requires failure in under
30 seconds. When adding Maven dependencies/plugins, publish a validated candidate
from successful preparation before rerunning normal/proof checks with a cold
cache. The scheduled/main refresh populates the shared release. A simultaneous
GitHub/upstream outage or an unpublished dependency addition is not covered.

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
