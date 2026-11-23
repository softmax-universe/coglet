# Mindister Coglet Architecture

Design notes from the November 2026 discussion with David. Mindister is the in-progress implementation; caos is inspiration and the eventual substrate.

## Vocabulary

- **Surface**: an adapter embedded in any host software (the Mac, Mindustry). It owns real-time I/O and grants capabilities.
- **Coglet**: semi-autonomous code that runs inside a surface and reads and writes only through granted capabilities. Mindister currently calls this an Appendage, and legacy code says Part.
- **Hub**: runs anywhere. Coglets dial in, register, and are controlled by the hub.

First test: a **Face** coglet on the Mac (hold to talk, it talks back) and a **Mindustry avatar** coglet. You talk to the face, the hub decides, and the hub drives the avatar.

## Networking

- iroh for everything. Coglets dial out to the hub, so this works from inside the game and behind NAT.
- Identity is the iroh node key. The hub keeps an allowlist of enrolled coglet node IDs.
- **One protocol, one ALPN.** Each operation (control, git sync, log tail, debug) opens its own QUIC stream on the same connection, so there is one handshake, one auth path and one versioned schema.
- Permissions are granted **per method**, not per protocol. Debug-read and debug-write are separate grants.
- Registration carries a manifest of the capabilities the surface granted. The hub configures itself from it, for example where speech-to-text runs.

## Coglet repo and git control plane

Each coglet is a git repo:

- `src/`: source, any language that compiles to Wasm.
- `bin/step.wasm`: the built component. **The hub builds it and commits it**, so the runtime only loads it.
- `prompts/`: prompt files for coglets with LLM access.
- `manifest`: the capabilities the coglet expects.

Branches:

- `refs/heads/hub`: the hub pushes code here. The runtime verifies the signature and switches at the next step boundary.
- `refs/published/main`: the coglet's signed published state (already in Mindister).
- `refs/heads/logs`: logs only, so log commits never conflict with hub code pushes.

Logs are JSONL appends under `/logs`, rotated by time or size and **batch committed** (about every 30 s or on rotation). Never commit per step. Fine-grained real-time logs go over the debug stream instead.

Private state in `/data` is never synced through git.

## Runtime

One Rust binary, the same on the Mac and next to the game, and language agnostic. The historical component/WIT sketch below is superseded by [Revised runtime model](#revised-runtime-model): v0 uses plain core Wasm on the WAMR interpreter, including on Macs and the hub.

### Step loop

Each tick:

1. Apply queued hub debug writes.
2. If `refs/heads/hub` moved, verify it and fast-forward. New code takes effect only here.
3. Take one pinned snapshot of the surface's devices.
4. Instantiate `bin/step.wasm` **fresh** and call `step`.
5. Journal the emitted intents, then dispatch them. World acts go through the hub's motor queue (pacing, player-wins), as in Mindister's `rt.act.request` path.

Steps can be event-driven (wake on an event) rather than fixed-rate.

### Component contract

- Export: `step(input) -> output`.
- Imports (WIT): one read per granted device, emit-intent, log append, LLM request. The inbox is not an import; it is a file (see "Inbox as a file").
- No wall clock or randomness except what the runtime passes as input, so steps are deterministic and replayable.
- Compute caps and deadlines follow [Budgets and engine conformance](#budgets-and-engine-conformance), not the historical Wasmtime fuel/epochs assumption.
- The runtime checks the manifest against the surface grants **before start** and refuses to run if anything is missing.

### Filesystem as state

- A fresh instance per step, but a persistent **in-memory WASI filesystem**: `/code` read-only (pinned commit), `/data` private, `/logs` append-only.
- Copy-on-write and content-addressed, so each step's state is a tree hash. Snapshots are cheap, the hub can diff them, and they map directly onto git and caos.
- Flushed to disk periodically for crash safety.

### LLM capability

- A capability import. Prompts live in the repo and the hub edits them like code.
- **Async**: a step sends a request and the response arrives in a later step's inbox, because LLM latency exceeds step deadlines.
- Every response is recorded with its step, so replay uses the recording instead of calling the model.

## Debugging

- Same protocol, separate streams: read `/data`, tail logs, inspect a step's inputs and outputs, pause, single-step.
- **Hub writes** to `/data` are queued and applied only between steps, never mid-step.
- Every hub write goes to a local audit log, so you can tell coglet behavior from hub intervention.
- Nothing streams unless the hub asks, so private data stays local by default.

## Face: full-duplex conversation

Audio cannot go through the step loop (frames arrive every 20 ms or so).

- **The surface owns real-time audio**: mic, echo cancellation, voice activity detection, streaming speech-to-text, text-to-speech playback, and the reflex of ducking or stopping playback when the user speaks.
- **The coglet owns decisions**, waking on events: speech start, partial transcript, end of utterance, playback position. It emits speak-chunk, stop and backchannel intents.
- **Barge-in has two layers**: the surface stops instantly, then the coglet decides whether to drop or resume.
- Speech-to-text and text-to-speech location stays agnostic: both are capabilities, so we can A/B on-device and hub-side.
- **The record keeps only what was heard.** On barge-in the surface reports how many milliseconds of each speak item actually played. The transcript and journal store the item truncated to that point, so the coglet never reasons about words the user didn't hear. (Companion truncates only the speech model's server-side item; storing the truncated item in the transcript and journal is our addition.)
- **Speech can hand work off.** A speech turn can start long-running work (an agent, a game action) as an explicit intent with an id, an epoch and size caps (Companion has the call id, work epoch and caps). Barge-in cancels it. The voice may report only outcomes the work's result event contains; Companion asks for this in its prompt, and we make it a requirement.
- **Hands-free and push-to-talk** are surface modes advertised in caps, not coglet logic.

## Mindustry avatar

- Mindister's game surface already exposes `/dev/game/{state,events,act,results,pointer,ctl}` to a supervised runtime over SP/1. These become WIT imports.
- Acts never execute locally. They relay through the hub's motor and back as fenced acts (unchanged).
- **The avatar's knowledge is data, not code.** It lives as files in the coglet's `/code` (read-only, versioned with the coglet) and `/data` (learned):
  - a **content index** generated from the pinned Mindustry engine source: blocks, units, items and liquids with cost, size, health, inputs, outputs and the defining source line;
  - **recipes**: request patterns mapped to the device calls that satisfy them, each with the source symbols involved, checked by tests against the pinned engine;
  - **activity ideas** for "what should we do?", offered as choices that fit the current game;
  - an **action library** learned from play: the hub groups journal entries of what the player asked and what was done into examples (Companion records the actions; the grouping and learning step is new). Promotion treats it like any other `/data` change.
- **Richer read devices** beyond raw state, each bounded and paged with a cursor, never full-map snapshots:
  - map find (blocks, ore, terrain, units; fog-respecting);
  - objectives and waves;
  - resources, keeping core inventory, carried items and campaign research pools apart;
  - the tech tree with partial funding;
  - logistics diagnosis (blocked or slow belts, with evidence and its strength);
  - navigation (menus, saves, destinations).
- **Write devices that don't touch the world**: highlights (labelled, expiring map markers, cleared on world change). Acts that do change the world (move, mine, build ranges, research, helper spawn and stop) go through the motor as usual.
- **The player's controls stay theirs.** A helper unit assists the player's own build plans and never replaces their ship. Avatar acts never clear human build queues (Companion's helper clears only its own queue; we make this an invariant with a test).
- **Live engine edits** (inspecting or setting engine fields, hot-reloading Java) are a debug capability behind hub policy, never a default grant.
- **Strategy is the gap.** None of the prior art covers build orders, wave defense or production scaling. The avatar learns those through the factory loop and records them in recipes, rather than hard-coding them.

## Game adapter: thin adapter plus local runtime process

Decision: the coglet does **not** run inside Mindustry's JVM. A thin in-game adapter talks over a local socket to the same Rust runtime binary the Face uses (WAMR interpreter in v0).

- **One runtime everywhere.** A Java coglet would mean a second runtime (for example Wasm on the JVM via Chicory), so every capability would be built twice and drift.
- **Crash isolation.** A hung or crashed coglet never takes down the game. The runtime can be restarted, hot-swapped or killed without touching the JVM.
- **Native iroh and git.** No JNI and no Java iroh bindings.
- **Matches what exists.** Mindister's game surface already supervises a separate runtime over a local SEQPACKET socket (SP/1) and serves the `/dev/game/*` nodes. Only the runtime itself is missing.

The adapter's job is small: project filtered game state, serve the device nodes, accept intents, and enforce grants and fencing. It holds no coglet logic.

Costs:

- The local socket hop was estimated at tens of microseconds per message. This is a microbenchmark-scale estimate, not a measured product latency guarantee; validate it with the real adapter workload.
- Snapshot serialization is the real cost, so send filtered state and diffs, not full state.
- One more process to supervise, which the existing supervisor already handles (startup, crash backoff, operator off-switch).

Revisit only if an coglet needs synchronous, per-tick access to engine internals that cannot be exposed as a device.

Repo changes:

1. Keep the SP/1 supervisor and socket as the game-to-runtime boundary.
2. Write the reference runtime that speaks SP/1, then map its device nodes onto the WIT imports.
3. Add snapshot diffing to `/dev/game/state` so per-step reads stay small.

## Hub control

All traffic flows through the hub: coglets never talk to each other directly. The hub is itself made of coglets, edited by the same factory, on top of a small fixed kernel.

### Structure

- **Kernel** (fixed, edited by hand only): runtime, git signature verification, promotion, health checks and automatic rollback.
- **Fast loop** (a coglet): a small fast model, always on. It takes face transcripts, keeps the conversation going, routes quick actions to the avatar, shuttles updates, and decides when a request needs new code.
- **Factory** (a coglet): writes or changes code for any coglet, including the fast loop and itself. Starts as a thin coglet over Hermes gateway and Kanban: tickets, worker profiles, prompts and review rules are files in its repo, while Hermes stays outside the self-editing loop. The interface stays generic so caos can replace Hermes later.

Risks with Hermes: Mindister's live factory is still disabled pending gateway qualification, and the local model helper rejects streaming.

### Ticket interface (fast loop to factory)

Ticket fields:

| Field | Contents |
|---|---|
| Target | Coglet repo and base commit |
| Goal | The request in plain words plus acceptance criteria written by the fast loop |
| Context | References to a world snapshot slice, recent transcript and relevant traces |
| Capabilities | What the work may touch (source and prompts, manifest) and debug grants (read, write) |
| Gate | Required checks: replay tests, headless game fork run, human approval |
| Budget | Time and spend caps |

Operations:

1. **Submit**: the fast loop files a ticket and gets an ID back immediately.
2. **Update**: the factory streams progress (planning, building, gate running, needs input). The fast loop narrates it to the user.
3. **Answer**: the fast loop relays the user's reply to a factory question.
4. **Amend or cancel**: change the goal or kill the ticket mid-job.
5. **Result**: done (signed candidate commit, gate results, spoken summary), needs input, failed or abandoned.

The factory never promotes. On done, the fast loop calls propose on the kernel.

### Promotion interface (kernel)

1. **Propose**: coglet ID, candidate commit and gate results. The kernel verifies the signature and checks the gates against that coglet's policy.
2. **Promote**: move the live branch to the candidate at the next step boundary, keeping the previous commit as last-known-good.
3. **Rollback**: revert to last-known-good, on request or automatically when health checks fail during probation.
4. **Status**: live commit, candidate, probation state and recent health per coglet.

Per-coglet policy is a file the kernel reads: required gates, whether human approval is needed, and probation length. Hub coglets (fast loop, factory) get stricter defaults, such as human approval early on.

### Debugging and steering

- Factory workers and the user debug coglets, always **through the hub**, never by connecting directly. The hub already holds the connection and enforces grants.
- A debug session is a capability on a ticket: debug read (tail logs, read `/data`, inspect step inputs and outputs, pause, single-step) or debug write.
- **Debug writes are allowed on live games**: the hub uses them to steer a coglet through complicated situations, not just to fix bugs.
- Writes land only between steps, and every one goes to the audit log, so traces separate coglet decisions from hub intervention (important for training data).
- The hub opens the streams for the ticket and closes them when the ticket ends.

### Inbox as a file

- The inbox is an append-only file, `/data/inbox.jsonl`. The hub appends messages between steps and never rewrites it.
- The coglet keeps its own read cursor in `/data`.
- One primitive covers everything: steering messages, async LLM responses and factory notes all arrive as hub appends to the inbox.
- Only the message format needs standardizing.

## Revised runtime model

These decisions supersede the earlier Runtime section where they differ: coglets are event-driven, the module is plain core Wasm, and the engine is swappable.

### Event-driven coglets

- A coglet runs only when there is something to handle. A step is "handle this batch of events", still on a fresh instance with state in files.
- Wake sources: surface events, inbox appends from the hub, LLM responses, and timers the coglet requests (for example "wake me in 500 ms").
- Periodic work, such as game ticks, is a surface event source the coglet subscribes to at the rate it wants.
- Events that arrive during a step are batched into the next one, with a cap.
- Replay records event batches instead of ticks. Idle coglets cost nothing, which helps phones and small devices.

### Plain Wasm and a swappable engine

- No component model. The contract is a plain core Wasm module with bytes (JSON or CBOR) across a few host functions, and plain WASI file access.
- The engine sits behind one small interface: load, set compute cap and deadline, wire host functions, call handle.
- **Build WAMR first, interpreter in v0 on all hosts, including Macs and the hub.** [ADR 0001](../adr/0001-engine-findings-cap.md) found interpreter instruction metering and watchdog interruption work with caveats; ahead-of-time (AOT) execution does not meter compute. Wasmtime remains a later engine behind the same interface; bringing it forward for fuel/epochs is an open option, not a decision. Cross-engine state-hash comparison is a required conformance goal, not an established guarantee for arbitrary guests.
- What WAMR gives up: Wasmtime's heavier fuzzing and auditing, some speed when interpreted, and newest-WASI features. None matter much for small decision-making coglets.
- The spike findings below constrain engine admission; they do not implement the production engine or freeze the ABI. ADRs 0001 and [0002](../adr/0002-engine-findings-platforms.md) remain Proposed.

### Budgets and engine conformance

- **Compute cap plus deadline:** among the tested WAMR modes, only the interpreter enforces both. Pin its build/version, enable instruction metering, reset a validated positive budget before each call, and use the watchdog interruption path. WAMR instruction units are engine/build-specific, not portable Wasm-opcode fuel. A future qualified metering engine (for example Wasmtime fuel plus epochs) may enforce both behind the same interface.
- **AOT restrictions:** ordinary and shared-memory AOT ignore the instruction-count limit. Ordinary AOT also ignores `wasm_runtime_terminate`; it cannot provide an in-process deadline. AOT is allowed only where neither cap is relied on, or as separately qualified shared-memory AOT for the deadline alone, never for a compute cap. The latter requires shared guest memory, the compiler's multi-thread safe points, and runtime thread-manager/shared-memory support (ADR 0001); it is not the v0 execution path or a decision to admit shared memory into the ABI.
- **Deadline scope:** watchdog interruption is scheduler-dependent, not a hard real-time bound or deterministic replay fuel. Bound or forbid start functions separately, and give host imports their own cancellation/deadline rules; the spike only caps exported calls and does not test blocking imports. Journal timeout outcomes without committing failed steps' partial effects.
- **Memory admission:** an engine **MUST refuse to start** a module whose initial linear memory exceeds its granted cap, before instantiation. It must **never silently raise the cap**. WAMR's page-limit setter alone is insufficient: ADR 0001 observed a cap of 4 pages raised to a module's initial 5 pages. Validate memory layout/count and imports before allocation, enforce growth limits, and account separately for stacks, native/module metadata and filesystem/process memory.
- **Required engine-suite cases:** reject initial memory above the cap without instantiating; accept initial memory at the cap; reject growth beyond the cap and overflowing growth without changing memory size; reject unsupported compute/deadline modes; exercise compute exhaustion, deadline interruption and fresh-instance recovery. Run the same suite against real and mock engines.
- **Timing evidence is microbenchmark evidence, not a latency guarantee.** ADR 0001's fresh-wake and paced-event timings and deadline-return samples cover a synthetic guest on one Linux x86_64 host, not end-to-end Face latency, EO1/iOS performance or many-coglet capacity. Product workloads still require acceptance measurements.

### Determinism and publication

[ADR 0003](../adr/0003-determinism-rules.md) is still **Proposed**, not an accepted determinism policy. Its publish-time float/NaN rewrite is a **hub control-plane build-then-commit step**, not a per-step runtime rewrite: the transformed module would be committed and its hash used as the hash of record. See the [hub protocol spec](../../spec/hub-protocol.md) for the protocol boundary (currently a placeholder; publication semantics remain to be specified). The proposed no-threads/no-shared-memory rule also means shared-memory AOT deadline support is not an implied permitted ABI mode. Cross-engine and cross-architecture determinism, including resource-limit outcomes, remains to be qualified.

### Platform notes

- **Macs and hub:** WAMR interpreter in v0 so both the compute cap and deadline are available. Earlier Wasmtime adoption remains an open option.
- **iOS:** planned WAMR interpreter-only execution, avoiding runtime machine-code generation/loading. **Unverified:** ADR 0002's Linux host had no Apple SDK, `xcrun` or Apple linker, so no iOS archive/link, simulator or device result exists. Archive/link on Xcode and a signed device harness remain open verification items. When suspended, the phone is intended to become a remote surface with coglets on the hub; distribution/downloaded-code policy needs separate validation, not a blanket personal/TestFlight assurance.
- **32-bit ARM (EO1) and other small devices:** WAMR interpreter. **ARMv7 verified only under QEMU**, not on a real EO1. ADR 0002's modern-sysroot binary requires GLIBC_2.34 and is not suitable for the frame's unmodified Ubuntu 18.04 baseline. An EO1-compatible sysroot/artifact and an identified physical EO1 run (including realistic native RSS/peak and runtime conformance) remain open verification items. Emulator memory measurements are not native-device guarantees.
- **Devices too small for a runtime:** become remote surfaces, with reflexes on the device and decisions on the hub.

### Many coglets per surface

- One runtime per device hosts many coglets. Each has its own Wasm instance, repo, filesystem, grants and identity.
- The surface offers everything it has; each coglet's manifest is checked against its own share.
- Coglets subscribe to the events they want, and the runtime fans events out.
- Reads are shared. Exclusive outputs, such as a speaker or screen, belong to one coglet at a time, assigned by hub policy.
- The hub promotes and rolls back each coglet separately. Coglets on the same device still talk only through the hub.

The name "surface" stays.

## Components to build

Two specs everything plugs into, then the pieces built on them. Any new surface or coglet only has to speak a spec.

### Specs

1. **Coglet ABI.** Plain Wasm module. Export `handle(events, step, logical_time, seed)`. Imports: read state, emit action, set timer, LLM request (answer arrives as a later event), log. Files: `/code` read-only, `/data` private including `inbox.jsonl`, `/logs` append-only. Manifest: ID, event subscriptions, required capabilities, exclusive outputs, budget.
2. **Surface protocol.** Local socket between a surface and the runtime. Messages: hello (capabilities with schemas: event sources, readable state, accepted actions), event, read/reply, act (immediate accept or reject, outcome later as an event), grants changed, health. The surface supervises the runtime process and owns the kill switch. Small SDKs in Swift, Go, Python and Rust.

### On each device

3. **Coglet runtime.** Rust sidecar hosting many coglets: engine interface (WAMR first), event router (subscriptions, batching, timers), copy-on-write filesystem per coglet, policy and budgets, replay journal (events in, actions out), git sync, hub link, debug agent (reads and writes applied between steps).
4. **Surfaces.** Native apps built on the SDK: Mac, iOS, Linux and Echo Dot (shared Go library), EO1, Mindustry. Peripherals such as EchoEar and the Z04 badge are capabilities of a host surface.

### Hub

5. **Hub protocol.** One iroh protocol, a stream per operation: enroll, coglet lifecycle, git, events up and actions down, debug, inbox append, health. Per-method grants.
6. **Kernel.** Fixed: identity and enrollment, coglet registry, routing, promotion (propose, promote, rollback, status), policy files, probation, action arbitration, audit log.
7. **LLM gateway.** Serves the LLM import: model routing, spend ledger, recorded responses for replay.
8. **Fast loop coglet.** Conversation, routing, filing tickets.
9. **Factory coglet.** Ticket interface over Hermes; builds Wasm and requests gates.
10. **Gate service.** Replays journals against a candidate and runs headless game forks.
11. **Trace store.** Journals, audit logs and LLM recordings, exported as training data.

### Tooling

12. **Coglet CLI and mock surface.** Scaffold, build, run locally, replay, attach a debugger.
13. **Network visualizer and simulator** (see below).

## Visualizer, simulator and mocks

One Mac app with two modes.

- **Live:** connects to the real hub as a debug client. Shows the hub, surfaces around it, coglets inside each surface, and links colored by health and latency, with events animating along them.
- **Simulated:** hub, runtimes and mock surfaces all run on the Mac, fed by recorded traces (a game session, a voice conversation). Supports fault injection: drop a link, kill a runtime.
- Clicking a coglet shows its live commit, probation state, `/data`, logs and a steppable journal. Factory tickets appear on the coglet they target.
- Build the live view first; it needs only the hub's status and debug streams.

### Mocks and contract tests

Every piece talks to its neighbors through a narrow protocol, so each has a fake that speaks the same protocol:

- **Mock surface:** replays recorded events, records actions.
- **Mock engine:** a native function standing in for a coglet, to test the runtime without Wasm.
- **Mock hub:** scripted links, inbox appends and promotions.
- **Mock LLM:** replays recorded responses.

One shared contract suite per protocol runs against both the real piece and its mock, so mocks cannot drift. The simulator is the mocks wired together, with real pieces swapped in one at a time.

## Moving Baby Bear onto this design

The coglet design replaces Baby Bear's architecture (the first prototype), migrated incrementally because Baby Bear runs the house daily.

1. **One shared runtime** for Mindister and Baby Bear, run as a sidecar on each pod.
2. **Pods become surfaces.** A pod's `device_agent_v1` capability list becomes coglet manifests; native handlers become capabilities. Realtime audio stays native.
3. **Pilot: EO1.** Split it into the surface (existing app minus decision logic: screen, sensors, signed updates), the coglet runtime, and a display coglet the factory can rewrite without an EO1 release.
4. **Reuse what works:** add coglet methods to the existing `babybear/rpc/1` iroh protocol rather than adding a protocol; grow `hub_runtime` deploy and rollback into per-coglet promotion; point Hermes dev cards at coglet repos.
5. **Hub last:** move the conversation route into a fast-loop coglet once the pattern is proven on pods.

Coglet code goes through kernel gates, not the merge train. Native code shrinks to surfaces and the kernel.

### Pod fit

From the repo docs, not a code-level audit of each pod.

| Pod | Kind | Notes |
|---|---|---|
| Mac | Standalone surface | Most work: delegation and job logic must move into coglets |
| EO1 | Standalone surface | Planned 32-bit ARM WAMR interpreter; QEMU only, physical EO1 verification open |
| Echo Dot | Standalone surface | Clean fit; already does audio; shares the Go pod library with Linux; another body for the Face coglet, using the LED ring instead of expressions |
| Linux | Standalone surface | Clean fit via the Go library |
| iOS | Standalone surface | Planned interpreter only and hub fallback when suspended; SDK/link and device verification open |
| EchoEar | Peripheral | USB mic, speaker and face on the Mac surface |
| Z04 badge | Peripheral | Bluetooth, through the Mac surface |

**Realtime voice:** Baby Bear's pods run a provider speech-to-speech session where the model drives the conversation. Treat that session as a surface capability: the coglet configures it, and its tool calls arrive as events.

## Prior art: Companion

`softmax-universe/companion` (studied at `d67f962`) is a Nim/Objective-C Mac app:
- a floating orb opens chat (on click) or voice (on right-click);
- chat goes to an agent over ACP;
- voice runs on a realtime speech model that hands app requests to that agent;
- each app plugs in through `apps/<id>/control.py` (`start`/`status`/`stop` plus typed actions, JSON in and out) and an in-app mod, extension or add-on (Mindustry, Chrome, Blender).

It has no runtime, hub, sandbox or specs: it is an all-in-one prototype of the Face and the desktop app, not of Coglet. We take its solved problems, not its architecture.

**Licensing:** Companion's own code and docs are Softmax IP, so we reuse them directly (copy, then adapt to Coglet's interfaces). The repo's GPLv3 licence covers the vendored Mindustry engine under `apps/mindustry/source/`, which is Anuke's; that part is not ours to relicense. The duplex audio code keeps its MIT Polyworld attribution.

| Companion piece | Where it lands in Coglet |
|---|---|
| `realtimes.nim` `interrupt(playedMs)`: truncate the spoken item to what played | speech capability (Face section above) |
| `realtimes.nim` `processWork`: staged, capped, deduped, cancellable voice-to-agent work | speech capability work hand-off |
| `duplex.m`, `devices.m`: one AVAudioEngine, VoiceProcessingIO echo cancellation that fails loudly, lock-free rings, playback generation flush, device change recovery | `surfaces/face-mac` audio kit |
| orb overlay, stable signing identity so mic permission survives rebuilds, microphone permission request, OFL fonts | Softmax desktop app shell |
| Mindustry mod hooks (`HookServer`: loopback, bearer token, bounded queue, game-thread dispatch, expiring requests) | game adapter's device server; the same shape suits any in-app adapter |
| `knowledges.nim`, `GAME_RECIPES.md`, `FUN_THINGS.md`, `PLAYER_REQUESTS.md` | avatar coglet knowledge files (Mindustry avatar section above) |
| `ActionJournal`, `PlayerRecorder`: bounded off-thread semantic recording, loss counters, attribution of requests to later acts | game surface events, journal and trace store; training export |
| `ContextHooks`, `MapFindHooks`, `LogisticsHooks`, `ResearchHooks`, `HighlightHooks`, `HelperHooks`, `NavigationHooks` | the richer `/dev/game` devices above |
| `apps/<id>/control.py` + `LOOKUP.md` per app; Chrome native-messaging bridge; Blender add-on | candidate future surfaces (open question) |

## Relation to caos

Inspiration now, substrate later. Matching ideas: git trees as values, content-addressed cached steps, compare-and-swap refs (`std/actor`), resident workers as a loop, and iroh tickets. Eventually each step can be a caos actor message: (code, data, snapshot, inbox) in, (data', intents, logs) out.

## Changes to the Mindister repo

1. **Add this doc** as `docs/design/coglet-runtime.md`, plus an ADR recording the decisions below.
2. **Rename to Coglet.** Replace Appendage and Part everywhere: `parts/`, `pp/0`, `PartLink`, `part_kind`, the `mindister/appendage/0` ALPN and the docs.
3. **Build the runtime**: WAMR interpreter behind the swappable engine interface, event-driven steps, copy-on-write memory filesystem, git sync, iroh client. The plan's repo layout supersedes this historical Mindister sketch.
4. **Define the plain core-Wasm ABI** for events, devices, intents, logs and LLM, replacing the earlier component/WIT sketch. Map the surface protocol to its host functions.
5. **Collapse the three ALPNs** (`enroll/0`, `appendage/0`, `git/1`) into one protocol with a stream per operation. Enrollment and git become methods, with per-method grants.
6. **Add debug methods**: data read and write, log tail, step inspect, pause and single-step, with queued writes and an audit log.
7. **Git**: keep the signed `refs/published/main` and `refs/heads/hub`, add `refs/heads/logs`, and remove the legacy `refs/heads/main` fetch fallback.
8. **Hub build step**: compile `src/` to `bin/step.wasm` and commit it before pushing.
9. **Face**: express speech-to-text and text-to-speech as capabilities in the manifest so the hub can pick their location by config.
10. **Cleanup**: delete the paused WebSocket server and the test-only WebSocket emulators once iroh peers cover them, and update the README, which still says nothing is built.

## Open questions

- Keep or cut the hub's ledger and motor journal? Pacing and player-wins for acts are likely still needed.
- Engine-specific compute and deadline budgets per step for each surface; whether to bring Wasmtime fuel/epochs forward (not decided).
- Physical EO1 verification with a compatible sysroot, and iOS SDK/archive/link plus signed-device verification (ADR 0002).
- Acceptance and qualification of ADR 0003's hub-side publish-time float rewrite, including cross-architecture determinism and resource-limit outcomes.
- Flush interval and size cap for the in-memory filesystem.
- Whether steps default to event-driven or fixed rate per surface.
- Desktop-app surfaces (Chrome via native messaging, Blender via an add-on) on the Companion pattern: a product decision for after the Mindister lane, not yet scoped.
