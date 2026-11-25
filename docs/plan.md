# Coglet build plan (v1, 2026-11-23)

Approved by David on 2026-11-23 ("write up the plan, spin out sessions, and put it into action").
Current design: [`docs/cogent-design/`](cogent-design/README.md), in the names of ADR-0020 and ADR-0031; `spec/` stays the source of truth for what is built.
Earlier design of record: [`docs/design/coglet-runtime.md`](design/coglet-runtime.md), copied from mindister main `0e7bd24bf`. Where the two differ, the newer pages win.
Review that shaped this revision: [`docs/design/plan-review-2026-11-23.md`](design/plan-review-2026-11-23.md).

## Goal

Build the coglet framework as cleanly factored pieces in the right repos:
- the framework abstractions and protocols;
- runtimes and adapters for Baby Bear and Mindister;
- a toy-robot simulator to play with;
- the test tools, mocks, visualizer and conformance suites that tie them together.

## Ground rules (Implementation context: every card follows these)

1. **Clean port of ideas, not code.**
   - Nothing from Mindister's `parts/`, `runtime/` or `pp/0`, the WebSocket emulator, the old COG/LET Python framework (now on branch `legacy/v0`, tag `legacy-v0`), or any compatibility shim. Retired names like Appendage and Part don't appear either.
   - Old material is reference only. Port test *cases* (crash-injection points, cartpole, duplex-audio, workflows W1–W12) as ideas rewritten against the new specs.
2. **Specs first.**
   - Every protocol has a spec in `spec/`, golden fixtures and a conformance suite.
   - Every real implementation and every mock passes the **same** suite.
   - The suite is a language-neutral black-box harness that drives an implementation over the wire. It is not Rust unit tests.
3. **Boundary rule.** Product-neutral pieces used by two or more products go in `coglet`. Anything tied to one product's world, data or hardware stays in that product.
   - **coglet:** Face (Mac adapter and face coglet), SDKs used by two or more products (Rust, Python, Swift, Go), the simulator and the visualizer.
   - **mindister:** the game adapter, the Java adapter SDK and Java codegen (which pin coglet releases), the avatar coglet and Mindister's router config.
   - **babybear:** the pod bodies.
4. **Spec changes go through an ADR** (`docs/adr/NNNN-*.md`). A card that needs a spec change stops, files the ADR as a PR, and links it.
5. **WAMR first, behind an `Engine` trait.** Wasmtime comes later as a conformance check; both engines must reach identical state hashes.
6. **Done when** names tests. Every card:
   - opens a PR to `softmax-universe/coglet` main with green CI;
   - gets an independent review, then merges;
   - finishes with `kanban_request_review`.
7. **Shared files.** Only the skeleton card creates the top-level `Cargo.toml` workspace, the CI workflows and `conformance/` layout.
   - Later cards add their own crate or directory.
   - Any shared-file edit must be minimal and rebased.

## Decisions

| Question | Decision |
|---|---|
| Old coglet repo | Moved to branch `legacy/v0` and tag `legacy-v0`; `main` is fresh history. Done 2026-11-23. |
| Visualizer | Web app served by the router or simulator; a Mac wrapper can come later. |
| Baby Bear protocol | Pods speak coglet's own router protocol on its own ALPN, served next to `babybear/rpc/1` while they move over. It doesn't extend `rpc/1`, which overrides design-doc §Moving Baby Bear item 4. |
| Worker capacity | 3 extra workers through Phase 1, then revisit at the freeze. |
| Face | Lives in coglet: `surfaces/face-mac/` (Swift) and `coglets/face/`. Persona and voice config stay in the product repos. |
| Kanban board | The `default` board, tenant `coglet`, with titles prefixed `coglet:`. Pool workers serve the default board. |
| Java adapter SDK | Lives in mindister: single consumer (game adapter). Decided by David 2026-11-24. |
| Softmax app adapter | The Softmax app (`apps/softmax-mac/`) exposes an adapter so coglets can see and control all app state. ADR-0030, David 2026-11-24. |
| Coglet storage | A plain filesystem (`/code`, `/data`, `/logs`), no Git repository inside a coglet. Coglets process events, accept patches and let their manager tail their logs. Git comes later as a Gitlet. ADR-0031, David 2026-11-25. |
| Manage port | The interface a coglet's manager uses is the **manage port** (`manage.*`), everywhere. It replaces the debug port, the `debug.*` methods and the name "back channel". ADR-0031, David 2026-11-25. |
| Mac test host | Real-Mac proofs run on the Mac mini, not the MacBook Pro. David 2026-11-24. |

## Repo layout (`softmax-universe/coglet`)

```
spec/
  conventions.md            CBOR profile (deterministic), id + capability-name grammar,
                            event/action envelope, error codes, versioning/negotiation
  abi.md                    handle(events, step, logical_time, seed); host fns; /code /data /logs (plain files);
                            /data/inbox.jsonl message format; journal + trace format
  surface-protocol.md       hello/caps, event, read/reply, act (accept→outcome), grants, health,
                            kill switch; transports: local socket AND in-process (embedded runtime)
  capabilities/speech.md    speech-start, partial, end-of-utterance, playback position, barge-in;
                            speak-chunk, stop, backchannel; STT/TTS placement; s2s sessions
  capabilities/expression.md
  hub-protocol.md           one iroh ALPN, a QUIC stream per op: enroll, lifecycle, events/actions,
                            manage port (patch, tail_log, read/write, pause/step, status, audit;
                            signed patches, build-then-commit), inbox, health; per-method grants.
                            The Git control plane is removed by ADR-0031 (Git later as a Gitlet)
  manifest.schema.json      id, subscriptions, required caps, exclusive outputs, budgets
  policy.schema.json        gates, approval, probation (frozen at M3, not v0)
  messages/                 CDDL + JSON Schema for every message
  fixtures/                 golden valid/invalid frames, scripted audio-event timelines
conformance/                language-neutral black-box harness + suites: abi/ surface/ hub/ engine/ speech/
codegen/                    schema → Rust / Python / Swift / Go types
crates/                     Cargo workspace
  coglet-types              generated
  coglet-engine             Engine trait (load, cap, deadline, wire, call)
  coglet-engine-wamr        interpreter + AOT; interpreter-only on iOS and the EO1
  coglet-engine-wasmtime    later
  coglet-fs                 copy-on-write, content-addressed FS, tree hashes, flush
  coglet-runtime            library (C ABI) + bin `cogletd`: event router, timers, batching, budgets,
                            journal/replay, patch apply, log tail, router link, manage agent
  coglet-proto              router protocol over iroh
  coglet-hub                kernel: identity/enroll, registry, routing, promotion, policy,
                            probation, action arbitration, audit
  coglet-llm                LLM gateway: routing, spend ledger, recordings
  coglet-gate               journal replay gates, headless forks
  coglet-trace              trace store + training export
  coglet-mock               mock engine / adapter / router / LLM
  coglet-cli                coglet new|build|run|replay|attach|push
sdk/
  guest-rust/               write coglets in Rust → wasm32-wasip1
  surface-rust/ surface-python/ surface-swift/ surface-go/
surfaces/
  face-mac/                 Swift: reusable audio kit (capture, echo cancellation, voice detection,
                            ducking) + hold-to-talk app
sim/
  worlds/gridbot  cartpole  arm2d  duplex-audio  paint
  scenarios/                recorded traces, fault-injection scripts
apps/visualizer/            web: live view (manage client) + simulated mode
apps/softmax-mac/           the Softmax app for Mac, with its own adapter (ADR-0030)
coglets/
  echo  counter  gridbot-pilot  face  fast-loop  factory (generic ticket API + mock factory)
  optimizer  memory  (in the design pages; not yet filed as cards)
release/                    cogletd binaries, SDK packages, harness, face-mac build; products pin versions
docs/adr/  docs/design/
```

**Other repos:**
- **mindister** owns:
  - the Java adapter SDK and Java codegen, which live in mindister and pin coglet releases;
  - the game adapter, which goes from SP/1 to the coglet adapter protocol, is written in Java on mindister's own Java SDK (which pins coglet releases), and must pass the conformance harness;
  - the avatar coglet;
  - its router config and policy, including the Hermes factory adapter and the fast-loop prompts;
  - the cross-repo test (Face → router → avatar) against released coglet artifacts.

  After cutover, delete mindister's `runtime/`, `parts/` and `pp/0`.
- **babybear** owns:
  - the pod bodies (EO1, Echo Dot, Linux via Go, Mac, iOS on the embedded runtime);
  - the Baby Bear coglets;
  - its router deployment.

  Mac audio follows an ADR: the Baby Bear Mac pod either adopts face-mac's audio kit or runs as a separate mode, never both at once.
- **coworld-mindustry** is unchanged.

## Phases and dependencies

```
P0  skeleton ──┬─ conventions spec ─┬─ ABI+journal spec ─────────┐
               │                    ├─ adapter + speech specs ───┤
               │                    ├─ router protocol spec ─────┤
               │                    ├─ policy draft              │
               │                    ├─ conformance harness ──────┤
               │                    └─ codegen ──────────────────┤
    spikes (parallel, no parents): WAMR cap+interrupt+event-rate,├─ v0 freeze (ADR-0009)
               WAMR armv7(QEMU)+iOS interp, cross-engine float hash┘        │
                                                                            ▼
P2 (all parallel): engine-wamr · mock engine · fs · runtime core(+C ABI) · guest SDK+echo/counter
   · adapter SDK rust+mock · python · swift · router proto+mock router · kernel part 1 · mock LLM+gateway
   · release pipeline
P3: M1 hello coglet → M2 sandbox (gridbot, cartpole, CLI, visualizer live) → M3 self-editing loop
    M2b Face on Mac (duplex-audio sim, then real Mac) runs beside M2
    parallel: Wasmtime + cross-engine hash, trace store, full LLM gateway, fault injection,
    visualizer simulated mode; policy freeze at M3
P4: Mindister lane (game adapter, avatar, fast loop, factory, Face→router→avatar, delete legacy)
    Baby Bear lane (EO1 pilot, Echo Dot/Linux via Go SDK, Mac pod + audio ADR, iOS embedded,
    conversation route → fast loop); each pod cuts over after a side-by-side check
```

**Notes on the graph:**
- **The v0 freeze** is the one real serial gate. It covers conventions, ABI, adapter, speech and the minimal router protocol. Policy freezes at M3.
- **Spec v0 isn't final.** M1 can still drive spec fixes through ADRs.
- **Phase 0–2 and M1 cards are filed now.** The orchestrator files M2, M2b, M3 and Phase 4 after the freeze (follow-up card `coglet:file-phase3-4`).

## Sessions

| Session | Owns |
|---|---|
| Coglet Orchestrator | The graph: sweeps every 20 min, catches idle lanes, files later phases, routes between lanes |
| Coglet Core | Specs (conventions, ABI), engine spikes, engine, filesystem, runtime, guest SDK |
| Coglet Hub | Router protocol spec, proto, kernel, LLM gateway, gate, trace, release pipeline |
| Coglet Tools | Conformance harness, codegen, mocks, SDKs, simulator, visualizer, CLI |
| Coglet Face | Speech/expression specs, Swift SDK, face-mac, face coglet; takes over from the Mindister Face session once its current acoustic proofs land, turning their evidence into acceptance thresholds |
| Mindister sessions (existing) | Finish the current Face and Game proofs, then the Phase 4 Mindister lane |
| Baby Bear Coglet | Starts at M1; Phase 4 Baby Bear lane |

## Pacing

- **Linux:** about 4 cards at once (worker-1 plus 3 autoscaled workers). Run the critical path first:
  1. skeleton, conventions, then the spikes;
  2. ABI, router protocol and the harness;
  3. at Phase 2, engine, runtime and router proto first.
- **Macs:** at most one Face card per Mac at a time. Swift compile and unit CI run on GitHub macOS runners; the Mac mini is the host for real-Mac and acoustic proofs.
- **Estimates** (unmeasured, likely optimistic):
  - specs and spikes: 2–3 days;
  - Phase 2: about 4–5 days;
  - M1–M3: week 2–3;
  - product lanes: weeks 3–5.
