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

One Rust binary, the same on the Mac and next to the game. It hosts a **Wasmtime component** and is language agnostic.

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
- Fuel caps compute per step. Epoch interruption enforces the deadline.
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

## Mindustry avatar

- Mindister's game surface already exposes `/dev/game/{state,events,act,results,pointer,ctl}` to a supervised runtime over SP/1. These become WIT imports.
- Acts never execute locally. They relay through the hub's motor and back as fenced acts (unchanged).

## Game adapter: thin adapter plus local runtime process

Decision: the coglet does **not** run inside Mindustry's JVM. A thin in-game adapter talks over a local socket to the same Rust/Wasmtime runtime binary the Face uses.

- **One runtime everywhere.** A Java coglet would mean a second runtime (for example Wasm on the JVM via Chicory), so every capability would be built twice and drift.
- **Crash isolation.** A hung or crashed coglet never takes down the game. The runtime can be restarted, hot-swapped or killed without touching the JVM.
- **Native iroh and git.** No JNI and no Java iroh bindings.
- **Matches what exists.** Mindister's game surface already supervises a separate runtime over a local SEQPACKET socket (SP/1) and serves the `/dev/game/*` nodes. Only the runtime itself is missing.

The adapter's job is small: project filtered game state, serve the device nodes, accept intents, and enforce grants and fencing. It holds no coglet logic.

Costs:

- The local socket hop is tens of microseconds per message, negligible next to a game tick.
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
- **Build WAMR first** (interpreter plus ahead-of-time compilation). Add Wasmtime behind the same interface later for the hub and Macs. Running the same coglet on both engines and comparing state hashes is a free conformance test.
- What WAMR gives up: Wasmtime's heavier fuzzing and auditing, some speed when interpreted, and newest-WASI features. None matter much for small decision-making coglets.
- Check early that WAMR can cap compute per step and interrupt a step that runs too long.

### Platform notes

- **iOS:** WAMR interpreter only, because iOS forbids generating or loading new machine code at runtime. When the app is suspended, the phone becomes a remote surface and its coglets run on the hub. Public App Store distribution may restrict downloaded code; personal and TestFlight installs are fine.
- **32-bit ARM (EO1) and other small devices:** WAMR, where Wasmtime support is weak. Verify the runtime builds and runs on the EO1 before relying on it.
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
| EO1 | Standalone surface | Clean fit; 32-bit ARM, WAMR |
| Echo Dot | Standalone surface | Clean fit; already does audio; shares the Go pod library with Linux; another body for the Face coglet, using the LED ring instead of expressions |
| Linux | Standalone surface | Clean fit via the Go library |
| iOS | Standalone surface | Interpreter only; hub fallback when suspended |
| EchoEar | Peripheral | USB mic, speaker and face on the Mac surface |
| Z04 badge | Peripheral | Bluetooth, through the Mac surface |

**Realtime voice:** Baby Bear's pods run a provider speech-to-speech session where the model drives the conversation. Treat that session as a surface capability: the coglet configures it, and its tool calls arrive as events.

## Relation to caos

Inspiration now, substrate later. Matching ideas: git trees as values, content-addressed cached steps, compare-and-swap refs (`std/actor`), resident workers as a loop, and iroh tickets. Eventually each step can be a caos actor message: (code, data, snapshot, inbox) in, (data', intents, logs) out.

## Changes to the Mindister repo

1. **Add this doc** as `docs/design/coglet-runtime.md`, plus an ADR recording the decisions below.
2. **Rename to Coglet.** Replace Appendage and Part everywhere: `parts/`, `pp/0`, `PartLink`, `part_kind`, the `mindister/appendage/0` ALPN and the docs.
3. **Build the runtime** (new `runtime/` Rust crate): Wasmtime host, step loop, copy-on-write memory filesystem, git sync, iroh client. This is the biggest missing piece; Mindister has no runtime today.
4. **Define the WIT world** (`runtime/wit/`) for step, devices, intents, logs, and LLM. Convert the SP/1 device nodes into WIT imports.
5. **Collapse the three ALPNs** (`enroll/0`, `appendage/0`, `git/1`) into one protocol with a stream per operation. Enrollment and git become methods, with per-method grants.
6. **Add debug methods**: data read and write, log tail, step inspect, pause and single-step, with queued writes and an audit log.
7. **Git**: keep the signed `refs/published/main` and `refs/heads/hub`, add `refs/heads/logs`, and remove the legacy `refs/heads/main` fetch fallback.
8. **Hub build step**: compile `src/` to `bin/step.wasm` and commit it before pushing.
9. **Face**: express speech-to-text and text-to-speech as capabilities in the manifest so the hub can pick their location by config.
10. **Cleanup**: delete the paused WebSocket server and the test-only WebSocket emulators once iroh peers cover them, and update the README, which still says nothing is built.

## Open questions

- Keep or cut the hub's ledger and motor journal? Pacing and player-wins for acts are likely still needed.
- Fuel and deadline budgets per step for each surface.
- Flush interval and size cap for the in-memory filesystem.
- Whether steps default to event-driven or fixed rate per surface.
