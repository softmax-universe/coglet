# Releasing Softmax for Mac

## Trust and credentials

The public repository is `softmax-universe/coglet`. Release builds use Developer ID
Application team **Z7589R323Y** (David's existing team), certificate SHA-1
`333D6D009F5B83C7CBB1C21439F7A34F3C070ECD`. Discovery found no Softmax-org
Developer ID in accessible AWS secret stores; this is the same certificate as Baby
Bear, **not** its update key. A valid leaf alone does not prove notarization; the
release job must pass every Apple check before publication.

Repository Actions secrets (never commit their values):

| Name | Purpose |
| --- | --- |
| `SOFTMAX_MAC_CERTIFICATE_P12` | Base64 P12 containing the pinned leaf and private key |
| `SOFTMAX_MAC_CERTIFICATE_PASSWORD` | P12 password |
| `SOFTMAX_MAC_APPLE_ID` | Apple account owning the app-specific password |
| `SOFTMAX_MAC_TEAM_ID` | Must equal Z7589R323Y |
| `SOFTMAX_MAC_NOTARY_PASSWORD` | App-specific password, validated by notarytool |
| `SOFTMAX_MAC_SPARKLE_PRIVATE_KEY` | Sparkle base64 32-byte Ed25519 seed |

The signing P12/password were sourced from the existing MacBook `~/.bb-signing/`
backup; notarization uses its existing app-specific Apple password. Baby Bear's
ASC key is an alternative, but cannot be read back from GitHub's write-only secrets.
Do not export other identities or rotate shared Apple credentials for this app.
The separate Ed25519 key is backed up at AWS Secrets Manager (softmax-org,
`us-east-1`) **`coglet/softmax-mac/sparkle-ed25519`**, JSON `private_key` and
`public_key`. Its public half is pinned in `Info.plist` and `release_policy.py`.
Re-run provisioning by reusing that secret, never silently generating a replacement.

Only the tag job receives signing secrets; pull-request proof jobs use disposable
update keys. The temporary keychain and P12 are removed in `finally`, and the prior
keychain search list is restored. GitHub masks Actions secrets; scripts must not
print secret-bearing subprocess arguments or errors. Hosted ephemeral runners are
intentional; do not send private signing material to untrusted self-hosted runners.

## Entitlements and runtime

`release.entitlements` contains exactly:

- `com.apple.security.device.audio-input`: microphone capture under hardened runtime;
  macOS TCC consent and `NSMicrophoneUsageDescription` are still required.
- `com.apple.security.network.client`: outgoing iroh/QUIC relay and peer connections.
- `com.apple.security.network.server`: incoming iroh peer/QUIC traffic.

The app is not App Sandbox enabled: the two network entries document intended
network capabilities and apply if sandboxing is introduced; they do not restrict
an unsandboxed app. No JIT, unsigned executable memory, get-task-allow or library
validation exemption is granted. All embedded Mach-O code, then nested bundles,
then the outer app are signed inside-out with `--options runtime --timestamp`.
`--deep` is used for **verification only**, not for signing. Project generation
remains unsigned for ordinary CI and development; do not alter development-only
`build-dev.sh` / `sign-dev.sh` to implement release signing.

## Workflow and proof

`.github/workflows/softmax-mac-release.yml` runs on `softmax-mac-v*` tags.
Accepted versions: `softmax-mac-vX.Y.Z`, optionally `-rcN` or `-betaN`.
`CFBundleVersion` is the monotonically increasing workflow run number (never rename
this workflow/reset its counter without a migration); display version comes from
the tag. Universal arm64/x86_64 archive targets macOS 14+. Sparkle framework and
tools are pinned to 2.10.1; the tools tarball is SHA-256 checked.

1. Generate the Xcode project and archive with signing disabled.
2. Configure bundle version and production-channel marker, sign inside-out, then
   run `codesign --verify --deep --strict --verbose=2`.
3. Submit a ZIP of the app with `notarytool submit --wait`, require **Accepted**,
   staple and validate the app, and assess the executable with `spctl`.
4. Build `Softmax-X.Y.Z.dmg` with create-dmg and Applications drop link. If the
   design supplies `apps/softmax-mac/assets/dmg-background.png`, include it.
   CI skips Finder scripting (`--skip-jenkins`), so Finder-specific icon layout
   is not an acceptance claim; polish can add a prebuilt `.DS_Store` later.
5. Sign the DMG, notarize it separately, staple and validate it, and run
   `spctl -a -vvv -t open --context context:primary-signature`. Apple documents
   this DMG policy in [Testing a Notarised Product](https://developer.apple.com/forums/thread/130560);
   `-t install` is the PKG policy, not the DMG gate. Both app and DMG signatures must verify.
6. Run actual Sparkle `sign_update` and `generate_appcast`, validate XML/version/
   URL/length/signature fields, then use `sign_update --verify` on the generated
   enclosure signature. Publish only after every check passes.

First production proof: create tag **`softmax-mac-v0.0.1-rc1`** on the independently
reviewed/green commit and push it. The release is a prerelease; download its DMG,
appcast and `verification.json`. The workflow log must show `Accepted`, successful
stapler validation, strict codesign verification and **Notarized Developer ID**
Gatekeeper acceptance. Do not equate an uploaded DMG with those checks.

Each PR also runs a real archive → ad-hoc signing → DMG → Sparkle interoperability
proof without secrets. `workflow_dispatch` with `adhoc=true` repeats that path,
uploading **Softmax-adhoc-proof** artifacts only, never a release or stable feed.
Ad-hoc output is explicitly non-notarized and unsuitable for end-user distribution;
it cannot satisfy Developer ID acceptance. There is **no automatic fallback** when
production secrets/notarization fail. If signing credentials become unavailable,
file the single specific credential ask described in the task, then use this
explicit proof mode while leaving production closed.

Portable checks:

```sh
python3 -m unittest discover -s apps/softmax-mac/scripts/tests -v
```

The separate `softmax-mac.yml` workflow compiles Swift and runs the core unit and
Settings XCUITest smoke. PR release proof exercises the production archive/signing
layout with ad-hoc identity, but only a tag run proves Apple notarization.

## Updates

The menu and Settings Updates tab share one `SPUStandardUpdaterController`.
Automatic checks default **off**; users can opt in. UI tests, local development
and ad-hoc builds never start the updater or make update requests. Only the
`developer-id` release marker enables it. EdDSA signatures are mandatory via the
pinned `SUPublicEDKey`.

Stable feed:
`https://github.com/softmax-universe/coglet/releases/download/softmax-mac-updates/appcast.xml`

Every release also carries its immutable appcast pointing to that release's DMG.
RC/beta tags never replace the stable feed. On a successful stable release, the
workflow uploads and downloads the dedicated feed asset and byte-compares it.
The feed is public and needs no GitHub credentials in the client. Before the first
stable release, the stable-feed URL is intentionally unavailable; RC users can
manually test their release's appcast but are not silently enrolled in a beta feed.
Keep the feed release as a prerelease so it does not become GitHub's "latest"
product release. Release builds are serialized per tag; do not concurrently publish
two stable versions out of order. To roll back, publish a **higher bundle build**
containing the known-good source; Sparkle rejects downgrades. Never replace the
public update key in installed apps without Sparkle's supported key migration.
