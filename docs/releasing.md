# Releases and product pins

Products such as Mindister and Baby Bear should pin an immutable **release tag
and source commit**, not `main`. Release versions do not change the frozen
`spec-v0` tag and are not protocol or hardware qualification claims.

## Build and publish

`.github/workflows/release.yml` builds the same inventory on a pull request
(changes to release tooling) or manual dispatch, without publishing. A push of a
root `vMAJOR.MINOR.PATCH` or `vMAJOR.MINOR.PATCH-rcN` tag publishes it; `N` starts
at 1. `v0.0.0-rc1` is the first dry-run prerelease, not a production endorsement.

1. Get independent approval of the exact PR head and green required CI; merge.
2. Check postmerge main CI, then create the release tag at that merged commit:
   ```sh
   git fetch origin main:refs/remotes/origin/main
   git tag -a v0.0.0-rc1 origin/main -m 'Coglet release pipeline dry run'
   git push origin refs/tags/v0.0.0-rc1
   ```
3. Follow the `release` workflow and link its successful run in the PR. All four
   binary jobs, source/wheel checks, SwiftPM tests and exact inventory sealing
   must pass before the publisher runs. The publisher rejects commits outside
   main history, uploads a draft Release, downloads the uploaded bytes and
   verifies their checksums, creates the matching Go submodule tag, rechecks
   both tag targets, then publishes.
4. Read back the Release's tag, prerelease flag, source commit and asset inventory.
   Never move a published version to repair a failed build. Fix forward using a
   new version. A failed publish can leave a draft or matching Go tag; inspect it
   rather than overwriting assets or retagging. Existing Releases are deliberately
   not overwritten by reruns.

No PyPI, crates.io or external Swift registry credentials are needed or used.
Only the publisher has `contents: write`; PR builds cannot publish. Normal CI,
review and branch protection still apply. Build logs and intermediate Actions
artifacts are retained independently of the durable GitHub Release assets.

## Inventory

For tag `vX.Y.Z[-rcN]`, every Release contains:

- `coglet-<tag>-{linux-x86_64,linux-aarch64,linux-armv7,macos-arm64}.tar.gz`:
  executable `cogletd` and `coglet-hub`. ARMv7 uses hard-float GNU/Linux and is
  smoke-tested under QEMU, not on EO1 hardware. Linux artifacts target the GNU
  libc/sysroot of Ubuntu 24.04; use a compatible deployment image and verify
  runtime library requirements for your target. They are **not musl/static binaries**.
  macOS binaries are arm64, unsigned and not notarized.
- `coglet-<tag>-rust-source.tar.gz`: complete Rust workspace and SDK crates,
  preserving path dependencies and `Cargo.lock`. These private crates remain
  `publish = false`; this is a buildable source distribution, **not individual
  crates.io `.crate` packages**. Use Git dependencies or the extracted workspace.
- `coglet-<tag>-swift-source.tar.gz` and `coglet-<tag>-go-source.tar.gz`:
  source-package snapshots. Root tag `<tag>` is a usable SwiftPM package;
  Go uses the additional `sdk/surface-go/<tag>` submodule tag at the same commit.
- `coglet_surface-X.Y.Z[rcN]-py3-none-any.whl` and corresponding `.tar.gz` sdist.
  The wheel's PEP 440 version is derived from the tag in a temporary source stage;
  source manifests are not rewritten in main. Runtime dependencies resolve
  normally when installing the wheel.
- `coglet-<tag>-harness.tar.gz`: runnable conformance harness, fixtures and adapter
  sources. Extract, then run `python3 conformance/run --self-test`. Running suites
  against a real implementation also requires building/provisioning its adapters
  and prerequisites as described in `conformance/README.md`; the bundle does not
  imply that all product/hardware implementations passed.
- `release.json`: source SHA, tag, consumer tags, qualification note, asset sizes
  and SHA-256 hashes. `SHA256SUMS` covers every payload and `release.json`.

The four source bundles deliberately contain the same complete Git snapshot.
This preserves relative workspace, generated-type and harness-adapter paths;
none includes uncommitted files, build outputs or local credentials. Archives do
not vendor Cargo/Python dependencies. Build toolchains and the pinned WAMR source
may require network access.

### Qualification limits

The default `cogletd` build packages the runtime's **reference/mock engine**, not
WAMR guest scheduling. The operational socket service is currently Linux-only;
macOS's binary exposes only the portable CLI/test-adapter modes. A macOS product
must embed the library/Swift SDK according to their documented contracts instead
of assuming this binary is a production socket daemon. Native-engine WAMR
`build.rs` currently qualifies Linux x86_64/ARMv7 only and rejects aarch64/macOS.
This release workflow does not silently change that boundary or label a
version-printing CLI smoke as native engine conformance. Check each component's
qualification/README documents before selecting it for a production product.

## Verify and pin

Download one Release's assets into an empty directory, including the manifest and
checksums; then run:

```sh
sha256sum -c SHA256SUMS          # Linux
# shasum -a 256 -c SHA256SUMS    # macOS
```

Record the Release URL, `release.json` commit SHA, selected artifact name and hash
in the product lock/config. Install binaries only after verification. Keep hub
identity/state outside the binary installation directory and preserve it during
upgrades. Releases do not bundle private keys, invites or product configuration.

### Rust

```toml
[dependencies]
coglet-runtime = { git = "https://github.com/softmax-universe/coglet", tag = "v0.0.0-rc1" }
coglet-surface-rust = { git = "https://github.com/softmax-universe/coglet", tag = "v0.0.0-rc1" }
```

Commit the product's `Cargo.lock`. The internal crate version remains `0.1.0`;
the release tag and locked Git revision identify the delivered source.

### Python

```sh
uv pip install ./coglet_surface-0.0.0rc1-py3-none-any.whl
```

Pin the wheel's verified Release URL/hash in the product dependency lock. Do not
expect `pip install coglet-surface==0.0.0rc1` from PyPI: this pipeline does not
publish to PyPI.

### Swift

```swift
.package(url: "https://github.com/softmax-universe/coglet", exact: "0.0.0-rc1")
// product: .product(name: "CogletSurface", package: "coglet")
```

The minimal root `Package.swift` points into `sdk/surface-swift/Sources`; commit
`Package.resolved`. Do not use the nested directory as a separate Git URL.

### Go

```sh
go get github.com/softmax-universe/coglet/sdk/surface-go@v0.0.0-rc1
```

Commit `go.mod`/`go.sum`. The publisher creates the required nested module tag
`sdk/surface-go/v0.0.0-rc1` at the root release commit. For a private repository,
configure authorized Git access and `GOPRIVATE` normally; no credentials belong
in a Release or a product lockfile.
