# CI dependency mirror

Status: proposed; implementation starts only after independent review and merge.
Scope: Linux `conformance` and `rust` first. No protocol or contributor
workflow change, so this is a design brief rather than an ADR.

## Decision

Use **input-addressed dependency bundles on GitHub Releases**, with Actions caches
as an optional accelerator and the existing public registries as fallback. This
is option (b), not a claim that an evictable Actions cache makes PRs never cold.
A trusted main-branch workflow builds bundles on dependency changes, daily and on
manual dispatch. A PR changing dependency inputs may start cold until its new
bundle is published after merge; fail with an actionable cache-miss message in
outage-proof mode, but use upstream normally. Existing dependency sets keep their
last good bundles even when scheduled refresh fails during an outage.

| Ecosystem | (a) Hosted pull-through | (b) Pinned bundle/vendor | (c) Actions cache + warm-up | Recommendation |
| --- | --- | --- | --- | --- |
| Cargo / crates.io | CodeArtifact now supports Cargo/crates.io external connections; Nexus/Artifactory support depends on product/version. Not a rustup mirror. | `cargo vendor --locked` is portable but large and needs source replacement. For CI, bundle registry index/cache/src and Git source caches populated by `cargo fetch --locked` for **every manifest exercised by the rust and conformance workflows**, including standalone guests and conformance driver. Lockfile checksums remain enforced. | Useful for speed; eviction, branch visibility and newly changed locks prevent a guarantee. Never rely on cached `target/` binaries as dependency proof. | (b): Cargo download/source closure, no committed vendor tree; proof uses `CARGO_NET_OFFLINE=true`. |
| rustup | No CodeArtifact rustup distribution upstream. A static S3 mirror needs manifests, component archives and upstream checksum verification; maintaining a full Rust distribution mirror is excessive. | Archive the installed exact Linux toolchain including clippy, rustfmt and wasm32-wasip1. Identity includes `rust-toolchain.toml`, OS/architecture, bundle schema and setup logic. No rolling `stable` channel. | Cache installed toolchain before invoking any command that auto-installs it; normal runner preinstalls do not prove availability. | (b), alongside Cargo: restore before `rustup show`; proof denies static.rust-lang.org too. |
| PyPI | CodeArtifact/PyPI supported; a proxy caches downloads but cold versions need upstream. | Wheelhouse/uv download cache keyed by requirements/uv locks, Python ABI and platform; install from local wheelhouse in a future full-PyPI-outage proof. Current requirements are not all hash-locked. | Cache pip/uv, scheduled warm-up; still cold on version/ABI changes. | (b) for the Python dependencies needed by these two jobs; resolve with the actual Python version. Do not claim all Python workflows are covered. |
| RubyGems | CodeArtifact now supports RubyGems; Nexus/Artifactory also possible. | `bundle cache` from `spec/tests/Gemfile.lock`, including platform variants needed by the Linux conformance job; restore before bundler installation. | `ruby/setup-ruby` bundler-cache already helps but can miss/evict. | (b) for conformance's locked gems; retain bundler-cache as accelerator. Ruby installation itself remains runner/setup-ruby infrastructure. |
| Go proxy | No listed CodeArtifact Go upstream; Athens/Artifactory is an extra service. | Bundle `GOMODCACHE` after `go mod download` keyed by `go.sum`, toolchain and platform, retaining checksum verification. | `setup-go` cache keyed by go.sum is simplest today, with warm-up on main. | (c) initially: not used by conformance/rust; no Go outage guarantee in phase one. Add durable module bundles when a Go outage proof is needed. |

## Storage, cost and authentication

Prefer Releases over an S3 tarball initially: this repository is public and needs
no new cloud account configuration or paid always-on service. Public GitHub-hosted
Actions usage and public release downloads use GitHub's public-repository offering;
check current plan/limits before assuming unlimited capacity. Measure compressed
bundle sizes and refresh duration in job summaries; split toolchains from source
repositories, keep individual assets below GitHub's 2 GiB limit. No runtime
outputs, credentials, Cargo credentials or arbitrary home-directory
archives belong in a bundle. Keep immutable versioned bundles; remove obsolete
sets only through an explicit retention policy, not during an outage refresh.

CodeArtifact is a reasonable future shared-org service: usage-priced storage,
requests and internet egress, no server administration. Budget from measured GB,
request volume and AWS region pricing, not an invented fixed monthly estimate.
A Nexus/Artifactory VM adds continuous compute, disk, patching, backup, monitoring
and commercial-license costs where applicable. S3 bundles cost storage, requests
and egress but need IAM/bootstrap work. Actions caches consume cache quota and
are evictable, even when daily warm-ups normally keep them hot.

Phase one uses ephemeral `GITHUB_TOKEN`: `contents: read` for consumers and
`contents: write` **only** in the publisher job. No long-lived secrets, no AWS
credentials and no OIDC grant are necessary. If moving to AWS, use GitHub OIDC
(`id-token: write`, audience `sts.amazonaws.com`), scoped trust for
`repo:softmax-universe/coglet:ref:refs/heads/main` (or a protected publishing
environment), a read-only consumer role and a separate publisher role. Request
short-lived CodeArtifact authorization tokens; scope IAM to the domain/repos
and `sts:GetServiceBearerToken` service condition. Never give untrusted fork PRs
publishing permission or use `pull_request_target` to run their code.

## Publication, trust and fallback

1. Derive independent keys for Cargo, Rust toolchain, Python and gems from
   their inputs, platform/tool versions and bundle preparation logic. Do not key
   on the full commit: unrelated source changes must reuse the same mirror.
2. Trusted publisher checks out main, populates clean dependency homes, runs the
   actual commands needed to close plugin/build/feature dependencies, then
   publishes archives plus SHA-256 manifest and source commit provenance under a
   versioned dependency release tag. Publish only after preparation succeeds;
   serialize writers, never overwrite a known-good set with a failed refresh.
3. Consumers validate identity/digests before extraction; reject absolute paths,
   traversal and escaping links. Manifests and archives on the same release give
   integrity/transport checks, not independence from a compromised publisher.
   Cargo lock checksums and toolchain upstream verification add package checks.
   PRs cannot publish shared bundles. Do not execute a restored binary
   cache from an untrusted PR's Actions cache in the privileged publisher.
4. Try durable bundles before commands that contact registries (especially
   `rustup`). On missing/unavailable/corrupt mirror, log the reason, discard any
   partial extraction, then use original upstream configuration with bounded
   retries. Do not silently weaken checksums or switch to an unrelated public
   mirror. Implement restore/setup fallback at the bundle layer, not by listing
   a second crate index.
5. If both mirror and upstream fail, fail clearly; no design can guarantee builds
   through simultaneous independent service outages or dependency additions never
   fetched before. GitHub outage survival is outside scope: Actions already needs
   GitHub checkout, workflow and action distribution.

## Demonstration and rollout gate

After this brief is reviewed/merged, add isolated bundle scripts/
local composite setup actions and a publisher workflow. Keep shared workflow
changes minimal, coordinate with their owner and rebase on main. Normal PR jobs
restore bundles and permit upstream fallback; local builds remain unchanged.
Contributors can optionally download a bundle using `gh` (or public asset URLs),
but need no AWS login, proxy config or vendored repository history.

The implementation PR must link a proof run executing the **same
conformance and Rust commands** as normal CI with restored bundles. A trusted
manual publisher may prepare the candidate bundle for a reviewed branch, but must
not overwrite main's published sets; candidate publication is explicitly scoped
and records its source SHA. Start proof on a fresh runner with Actions dependency
caches disabled/cleared, restore first, then deny
`crates.io`, `index.crates.io`, `static.crates.io` and
`static.rust-lang.org` (both IPv4/IPv6; remove proxy bypasses). Demonstrate failed
HTTP access to those endpoints, set Cargo offline, and run the normal rust and
conformance commands. Do not fetch dependencies immediately
before blocking on the proof runner. Python/Ruby/bootstrap actions may still use
their upstreams: report that limitation, not a whole-internet outage claim.

Required evidence: bundle keys/digests/source SHA, fresh restore receipts, denied
endpoint probes, green conformance + rust, normal required CI, independent
exact-head implementation review, and merge of that reviewed head. Exercise
restore miss/corruption/fallback in script tests. Refresh failures leave last-good
bundles available and an actionable failed workflow, not a green empty publisher.

## References

- [CodeArtifact external connections and supported ecosystems](https://docs.aws.amazon.com/codeartifact/latest/ug/external-connection.html)
- [CodeArtifact pricing](https://aws.amazon.com/codeartifact/pricing/)
- [GitHub dependency caching: scope, limits and eviction](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)
- [GitHub OIDC with AWS](https://docs.github.com/en/actions/security-for-github-actions/security-hardening-your-deployments/configuring-openid-connect-in-amazon-web-services)
- [GitHub release asset limits](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)
