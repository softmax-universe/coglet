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
| `SOFTMAX_MAC_ASC_KEY_ID` | App Store Connect API key ID |
| `SOFTMAX_MAC_ASC_ISSUER_ID` | ASC issuer UUID |
| `SOFTMAX_MAC_ASC_PRIVATE_KEY` | Matching ASC P8 private key |
| `SOFTMAX_MAC_TEAM_ID` | Must equal Z7589R323Y |
| `SOFTMAX_MAC_PROVISIONING_PROFILE` | Base64 Developer ID profile authorizing `Z7589R323Y.com.softmax.app` and the pinned certificate |
| `SOFTMAX_MAC_SPARKLE_PRIVATE_KEY` | Sparkle base64 32-byte Ed25519 seed |

The signing P12/password were sourced from the existing MacBook `~/.bb-signing/`
backup; notarization uses the existing ASC key selected by
`~/.appstoreconnect/asc.env`, independently validated with `notarytool history`
before provision. The initial app-specific-password probe failed; it is not used.
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
validation exemption is granted. For Developer ID releases only, `release.py`
requires and embeds the CMS provisioning profile and adds its exact authorized
`com.apple.application-identifier` and `com.apple.developer.team-identifier` to
the outer app signature. The application identifier is the private default Data
Protection Keychain access group; no explicit shared Keychain group is enabled.
Team, prefix, bundle ID, expiration and pinned certificate are checked before
signing. The profile is ASC `MAC_APP_DIRECT` ID `9LA27AHTVY`, bundle registration
`F5QWV5467H`; the existing certificate is reused, not rotated. Ad-hoc builds keep
only the three base entitlements and cannot prove production Keychain access.
See Apple's [TN3137](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains).
All embedded Mach-O code, then nested bundles,
then the outer app are signed inside-out with `--options runtime --timestamp`.
`--deep` is used for **verification only**, not for signing. Project generation
remains unsigned for ordinary CI and development; do not alter development-only
`build-dev.sh` / `sign-dev.sh` to implement release signing.

## Local development Hub identity (Debug only)

Run `bash apps/softmax-mac/scripts/build-dev.sh`, then open the printed app path.
`sign-dev.sh` uses the selected stable developer certificate or its explicit ad-hoc
fallback; neither path claims production Data Protection Keychain entitlement.
Debug builds instead use the visibly labelled **Development identity** store at
`~/.softmax-development-hub/` (directory 0700, identity/enrollment files 0600).
This is an unencrypted, disposable, local developer identity, not a production
credential store. Use only test routers. It never reads, copies or migrates the
production Keychain or production enrollment preferences. Release builds compile
out this backend and select the unchanged `KeychainHubStore`: Data Protection
only, AfterFirstUnlockThisDeviceOnly, non-synchronizable, no legacy fallback.

The developer store preserves identity across rebuilds/relaunches, refuses key
replacement and insecure/symlink storage, and fails closed on corrupt identity or
an enrolled device whose identity is missing. Sign out explicitly to remove that
development identity/enrollment; it does not revoke router-side authorization.
Never delete identity alone to repair enrollment. Invitations are not persisted.

For manual local mock enrollment, build `cargo build --locked -p coglet-mock
--example hub`, copy the public device ID from Settings > Hub, and start
`target/debug/examples/hub DEVICE_NODE_ID` with stdin held open. Its single stdout
JSON line is private bootstrap material, not a log. In **Pair router… > Developer:
manual router address**, enter its hub ID, addresses and invitation, plus a test
name. Do not paste invitations into bug reports. Sending `d` to the launcher's
stdin closes the **hub side** of the active connection while preserving enrollment;
the app must become offline and reconnect with the same public ID. EOF stops the
mock. `HubNativeConnectionTests` exercises this fault and persisted development
identity; `DevelopmentHubStoreTests` checks storage isolation/fail-closed behavior.

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
4. Build `Softmax-X.Y.Z.dmg` with create-dmg and Applications drop link. Use
   `apps/softmax-mac/Design/DMG/background@1x.png` when available, 660×440 content,
   app at (180,230) and Applications at (480,230), matching the design record.
   Native proof exercises Finder layout scripting; visual inspection remains useful.
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
product release. Tag builds share one global publication concurrency group; do not
publish older stable semantic versions after a newer one. To roll back, publish a **higher bundle build**
containing the known-good source; Sparkle rejects downgrades. Never replace the
public update key in installed apps without Sparkle's supported key migration.
