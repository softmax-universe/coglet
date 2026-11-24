# Baby Bear iOS embedded-runtime rollout

Owner: `t_01a2084e0e33453d`. This is the integration plan and evidence boundary,
not a completed product rollout.

## Producer boundary

`bash scripts/build-ios-runtime-xcframework.sh` builds two distinct static-library
slices: `aarch64-apple-ios` with iPhoneOS, and `aarch64-apple-ios-sim` with
iPhoneSimulator. Both are arm64 and target iOS 15.0. Intel iOS simulators are
not supported. The existing universal macOS builder is unchanged. WAMR remains
interpreter-only; no AOT/JIT policy changes or protocol extensions are involved.

The builder runs cbindgen, compiles the real native-engine runtime, verifies each
architecture, refuses fault-injection symbols, links the exported C ABI against
both SDKs and verifies each linked platform with vtool. It then packages both
libraries as `CogletRuntime.xcframework`. `COGLET_IOS_XCFRAMEWORK_OUTPUT` chooses
an output directory; an existing framework is refused rather than overwritten.
`--plan` prints the finite build matrix without requiring Xcode or writing files.

The `ios-runtime` job in WAMR platforms builds this archive on a GitHub macOS
runner and retains the framework, symbol inventory and link diagnostics. That
job is **archive/link evidence**, not simulator or device execution. The three
Python source/admission tests are not compiler evidence. CI execution must be
recorded before accepting a target; Linux engine tests cannot establish it.

The current immutable `v0.0.0-rc1` Release has no xcframework and predates Apple
qualification PR62. Do not change its tag or upload a replacement under it. Before
Baby Bear consumes a framework, publish a new immutable release containing these
slices and pin release URL, source SHA and asset SHA256 in the product. An Actions
artifact is useful build evidence, not a substitute for that product pin.

## Product integration and side-by-side contract

The Baby Bear implementation belongs in `softmax-universe/babybear`, alongside
`pods/ios/Sources/BBIOSPodRuntime` and the host app under `ios/`. Framework-neutral
build support stays in coglet. Keep the existing pod identity, hub ownership,
permissions, unknown-command receipts and background/foreground protections.
Never copy credentials or private production recordings into hosted tests.

1. Keep native pod behavior selected by default. Feed the same bounded, synthetic
   lifecycle/command trace to the native decision path and an embedded Wasm guest.
2. During shadow comparison, only the native path may actuate hardware, publish
   commands or return production receipts. The shadow produces comparison intents
   only; it must not open a second microphone, speaker or network command owner.
3. Compare canonical decision/action projections and final state for foreground,
   background, unlock, network loss/recovery, revoked permissions, duplicate IDs,
   unknown outcomes and guest failure. Retain exact source/guest/artifact hashes
   and the per-case results; a fixture-generated expected result is not an
   observed side-by-side comparison.
4. Permit selection only after the recorded comparison matches. One selected
   executor owns actions at any point. Rollback closes embedded links, stops the
   runtime and selects the preserved native lane without replaying historical
   unknown or already-dispatched commands. Guest failure fences new execution;
   it must not silently redeliver through the native path.
5. Exercise the actual Baby Bear app's unit/UI targets on its native simulator CI
   path, with positive named execution and no skipped requested cases. Include
   durable suspend/resume and rollback. This does not establish physical-phone
   sandbox, audio or background-runtime qualification.

## Landing gates still open

- Actual iOS framework build/SDK link CI and independent exact-head review.
- New immutable framework release and verified Baby Bear pin.
- Embedded Baby Bear guest and native/shadow/selected ownership wiring.
- Recorded side-by-side parity and rollback in native iOS unit/UI execution.
- Required CI plus optional `apple`, `macos` and `swift` green on the reviewed
  head, normal merge gates and final same-card review. No EO1 pilot dependency.
