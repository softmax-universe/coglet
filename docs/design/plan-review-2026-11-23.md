# Coglet build plan review (revision 2: Face moves into coglet)

Read-only review. Sources: coglet-runtime.md (design doc), `babybear/pods/*` checkout, Mindister protocol notes.
Not inspected: Mindister's current Face code (this review does not say how it is built).
Pacing comments are judgment, not measurement.

## Effect of the Face move (summary)

- Repo boundary rule to adopt: product-neutral and reused by 2+ products goes in coglet. Tied to one product's world, data or hardware stays in that product.
  - Face passes the rule. Mindister and Baby Bear both want it, and the design doc makes the audio stack and conversation behavior generic (Face section; Pod fit: Echo Dot "another body for the Face coglet").
- Face is the first real consumer of surface realtime audio, so it forces an audio capability spec that was not in the plan.
- Face-mac is Swift, native audio and Mac-only. That pulls the Swift SDK and Mac cards onto the critical path, reversing my earlier "defer Swift to Phase 4".
- Baby Bear's Mac pod already has its own Swift audio stack (`pods/macos/Sources/BBMacCapabilities/`: `PodVoice`, `RealtimeRoom`, `EchoLeakMeter`, `RealtimePushToTalk` tests, `PodRealtimeHost`, `NativeCaptureStartup`). Two Mac audio stacks on one machine will fight over the mic and speaker unless the reuse path is planned.

## Ranked improvements (14)

1. **Prove the risky engine facts before the freeze (unchanged).**
   - Change: Phase 0 spikes, run in parallel with the specs, for these items:
     - WAMR per-step compute cap and deadline interrupt.
     - WAMR on 32-bit ARM, with QEMU armv7 as the CI fallback.
     - WAMR interpreter build for iOS.
     - Cross-engine hash with floating-point.
   - Why: design doc, Revised runtime model ("Check early that WAMR can cap compute per step") and Platform notes ("Verify the runtime builds and runs on the EO1"). The plan proves these in Phase 2 and Phase 4, after the ABI freeze.
   - Impact: avoids unfreezing ADR-0001.
   - New with the Face move: also spike a Wasm coglet making decisions at event rates matching speech events (partial transcript every ~100 ms). That confirms the fresh-instance-per-event step cost is acceptable for Face.

2. **Add a "common conventions" card before the four spec cards (unchanged).**
   - Contents: deterministic CBOR profile, id and capability-name grammar, event and action envelope, error codes, versioning.
   - Why: the four cards write into the same `spec/messages/` and `manifest.schema.json`.
   - Impact: avoids naming drift and merge collisions at the freeze.

3. **NEW: add a speech and expression capability profile spec.**
   - Contents: a spec file, e.g. `spec/capabilities/speech.md` (and `expression.md`), defining:
     - Events: speech-start, partial transcript, end-of-utterance, playback-position, barge-in notice.
     - Actions: speak-chunk, stop, backchannel, set-expression.
     - Config: where STT/TTS runs (device or hub). The design doc says both are capabilities so location can be A/B tested (Face section).
     - Provider speech-to-speech sessions: the design doc says to model the BB provider session as a surface capability whose tool calls arrive as events (Pod fit, "Realtime voice").
   - Why: with Face in coglet, `coglets/face` must run unchanged on Mac, Echo Dot and EO1 bodies. That only works if bodies differ behind one capability contract. The abstract "surface protocol" alone does not define audio semantics.
   - Where it fits: it is part of the hub/surface specs, written in the spec phase and covered by conformance fixtures. The `duplex-audio` sim world is its mock.
   - Impact: highest of the new items. Without it, `face-mac`, the Face coglet and BB bodies each invent audio semantics.

4. **Add the specs the plan never lists (unchanged).**
   - Specs to add: git control plane (refs `hub`, `logs`, `published/main`, signing, build-then-commit of `bin/step.wasm`), debug streams, inbox message format, journal and trace format, ticket interface.
   - Where: debug, git and inbox go with the hub-protocol card. The journal format goes with the ABI card. The ticket interface goes in the Phase 3 filing.
   - Why: design doc sections Coglet repo and git control plane, Debugging, Inbox as a file, Visualizer. Replay, gate, trace store and visualizer depend on them.

5. **Shrink the freeze gate (unchanged).**
   - Freeze ABI, surface protocol, the minimal hub protocol and the conventions as v0 only after the M1 slice runs.
   - Freeze policy and promotion at M3.
   - Why: the freeze is the plan's only serial gate. Specs not exercised by code are the likeliest to be wrong.

6. **Make the runtime embeddable as a library with a C ABI, not only the `cogletd` binary (unchanged).**
   - Why: the doc calls the runtime a "sidecar", but iOS cannot spawn one (Platform notes, iOS). The surface protocol then needs an in-process transport as well as a local socket.
   - Face makes this more pressing: a Face on iOS later means the audio surface and runtime share a process.

7. **Language-neutral black-box conformance harness (unchanged).**
   - It drives any surface, runtime or hub over the wire with golden fixtures. It must not be Rust-only unit tests.
   - Face addition: speech-capability fixtures include scripted audio-event timelines and barge-in cases, with no real audio needed in CI.

8. **Reorder around Face: Swift SDK and `face-mac` move up, as a parallel lane after M1.**
   - Change: after the freeze, run the Swift surface SDK in Phase 2 (not Phase 4), and add milestone M2b: "Face on Mac".
   - Exit criteria for M2b:
     - Face in simulated mode against the `duplex-audio` world, with scripted barge-in.
     - Then the real Mac: hold-to-talk, spoken reply, barge-in.
   - Why: Swift is the only Mac path. The Mac slots are scarce (one card at a time on mbp and mac-mini, which also run existing Face and Game proofs), so the Swift SDK card must be filed early enough to queue. This replaces my earlier "defer Swift" advice for the Swift SDK only. Go stays deferred to Baby Bear.
   - Constraint: use GitHub macOS runners for Swift compile and unit CI. Reserve mbp for the acoustic proof (real mic, echo cancellation, latency), because CI cannot hear.

9. **NEW: plan the Baby Bear Mac audio overlap now.**
   - Facts: BB's Mac pod already owns a Swift audio stack (see summary). `face-mac` will own another.
   - Change: split the new Mac audio code into a Swift package inside coglet (`surfaces/face-mac/` exposing a reusable audio kit: capture, echo cancellation, VAD, playback ducking). Record an ADR: BB's Mac pod adopts that kit or runs as a separate mode and never both at once. Exclusive outputs (mic, speaker) are assigned to one coglet at a time by hub policy (design doc, Many coglets per surface).
   - Do not rewrite or delete BB's audio stack in this plan; the BB cutover decides.
   - Impact: avoids a mic/speaker contention bug and duplicate effort.

10. **NEW: move the existing Face audio proof work to a clean handoff.**
    - Why: Face and Game Mac proofs are running now in Mindister sessions. Moving Face to coglet means the proofs either finish where they are, then get ported as ideas, or restart.
    - Change: name one rule in the orchestrator: finish the current acoustic proofs, record the evidence (samples, latency numbers) as fixtures and thresholds in the coglet conformance and acceptance docs, then freeze Face work in Mindister. No parallel development in both places.
    - Port ideas, not code, per the clean-port rule. The Mindister Face audio-proof evidence becomes the acceptance bar for `face-mac`.

11. **Cross-repo release and pinning (stronger now).**
    - Change: add an early card for coglet release artifacts: `cogletd` binaries, SDK packages (Rust crate, Python wheel, Swift package, Go module), the conformance harness, and the face-mac app build. Mindister and Baby Bear pin versions.
    - Why: with Face in coglet, Mindister's integration (talk to Face, hub drives avatar) crosses repos. Cutover and rollback need pinned versions.
    - Where the cross-repo integration test lives: in Mindister (consumes released coglet artifacts and runs Face-to-avatar), so coglet CI never needs the game.

12. **Java surface SDK or Mindister-owned Java client (unchanged).**
    - Why: the game adapter stays in Mindister and is JVM (Game adapter section). It is the only Java consumer, so Mindister owns it, but it must pass the coglet conformance harness. Make that explicit in the plan.

13. **Baby Bear protocol decision now (unchanged), and Face affects it.**
    - Decision: BB pods speak the coglet hub protocol on their own ALPN, served beside `rpc/1` during transition. The design doc says to extend `rpc/1` (Moving Baby Bear, item 4), which conflicts with the plan's "no shims".
    - Evidence: `rpc/1` appears in `pods/eo1/native/src/bin/bb-pod-iroh.rs`, `pods/echoear/firmware/src/hub.rs`, `pods/ios/acceptance`.
    - Face effect: Echo Dot and EO1 become Face bodies, so their transport choice affects Face speech and expression events. Where the Rust hub kernel runs for Baby Bear (BB's hub looks like a Worker, `wrangler.jsonc`) is unverified.

14. **Pacing and capacity.**
    - Keep 3 extra workers for Phases 0 and 1, then raise to 8 at the freeze, when about 12 independent Phase 2 cards plus the Swift, Face and speech-fixture cards exist.
    - Mac work: queue at most one `face-mac` card per Mac at a time. Plan about 2 Mac cards in flight.
    - Estimates are unmeasured; real audio proofs are the least predictable item.

## Where Face sits in the layout

```
surfaces/
  face-mac/          Swift Mac surface: audio kit + surface-protocol client, hold-to-talk app
coglets/
  face/              turn-taking, barge-in decisions, speak/stop/backchannel; persona prompts via product config
spec/capabilities/   speech.md, expression.md (new)
sim/worlds/duplex-audio   mock body for Face (exists in plan)
```

- Persona prompts and voice choices stay in product repos (Mindister hub config, BB config). The Face coglet keeps only mechanics.
- Echo Dot (LED ring) and EO1 (screen expressions) stay in `babybear/pods/*` as bodies. They implement the speech and expression profiles and run `coglets/face` unchanged. Echo Dot is the "another body for the Face coglet" case in the doc.

## Should other surfaces move the same way?

- **Game adapter (Mindustry, Java): stays in Mindister.** One product, tied to the game world.
- **Avatar coglet and hub config: stay in Mindister.**
- **EO1, Echo Dot, Linux, iOS pods: stay in Baby Bear.** Hardware and product specific. They must pass the coglet conformance harness. The Go pod library can later be promoted to `sdk/surface-go` if the Linux and Echo Dot code shares it.
- **EchoEar and Z04: stay in Baby Bear** as peripherals of the Mac surface (Pod fit).
- **`face-mac`: in coglet,** as a reference surface. No other surface needs to move now.
- **BB Mac pod audio: do not move yet** (item 9).

## Effect on sessions

- Add a "Coglet Face" session in coglet: Swift face-mac, speech capability, face coglet. It takes over from the Mindister Face session after the proofs finish (item 10).
- Mindister sessions narrow to game adapter, avatar coglet and hub config.
- Baby Bear Coglet session consumes Face for EO1 and Echo Dot, and owns the Mac audio overlap decision with the Face session.
- Orchestrator tracks one new cross-repo dependency chain: speech spec, then Swift SDK, then face-mac, then Face coglet, then Mindister integration.

## Effect on ordering

- Spec phase gains the speech and expression capability spec; Phase 0 spikes gain the event-rate spike.
- Swift SDK moves from Phase 4 into Phase 2.
- New milestone M2b (Face on Mac) after M1 and before M3.
- Mindister product lane (avatar and goal talk) now depends on a coglet release that includes Face.
- EO1 and Echo Dot Face bodies become late Baby Bear Phase 4 cards, after face-mac proves the speech spec.

## Adversarial pass on this revision

Items challenged and kept:
- Item 3 (speech spec): survives. Without it, three implementers define the audio semantics.
- Item 8 (Swift SDK earlier): survives, but only for the Swift SDK. The earlier "defer Go" stays.
- Item 9 (BB Mac audio overlap): survives as an ADR plus a planned reuse point. I dropped my first idea of rewriting BB's audio stack, since it is out of scope and risks the daily-use house.
- Item 11 (releases and pinning): survives. It follows directly from a three-repo plan.

Items dropped or softened:
- Dropped: "promote the Go pod library to coglet now." Echo Dot and Linux already share it in BB, and moving it has no evidence of benefit yet. Revisit only if a second consumer appears.
- Dropped: "move BB's Mac pod into coglet." It is product-specific and in daily use.
- Dropped: "add a hardware audio rig to coglet CI." Costly, and the mbp acoustic proof plus scripted audio-event fixtures cover it.
- Softened: the Face-in-sim claim. The `duplex-audio` world proves timing and event ordering, not acoustics.

## Already right, do not change

- Specs first, with a mock per protocol passing the same suite.
- Generic framework in coglet, product surfaces in product repos (the Face move follows this).
- Runtime core on the mock engine so it does not wait for WAMR.
- Legacy main to `legacy/v0`, fresh main.
- Phases 3 and 4 filed after the freeze.
- Wasmtime and cross-engine hash as a later parallel check.
- Thin game adapter as a separate process.
- Realtime audio staying native in the surface; the Face coglet decides, the surface reflexes (Face section).
- Phase 0 EO1 instinct ("proved on device first"), pulled earlier by item 1.
