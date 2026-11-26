# Runtime core and native surface service

Installed router pushes activate at a serialized runtime boundary, preserve
private data and reconcile the original push receipt across daemon crashes.
See [installed push qualification](installed-push-qualification.md) for custody,
named tests, native/default distinctions and finite storage bounds.

This crate is **not yet an M1-qualified runtime**. The current library provides:

- arrival-ordered bounded batching (`Router`);
- validated snapshot overlays with guest mount restrictions;
- a transactional host for mock `emit`, `timer`, `llm`, and `log` imports;
- independent host return calculation, lifetime effect IDs, atomic guest rollback;
- canonical ABI step records and input-driven mock replay (never output injection);
- an exclusive, fsynced, checksummed WAL with partial-tail recovery;
- a durable coordinator that persists event admission and step consumption together.
- persistent pending timers, due-wake queueing and causal one-shot consumption;
- pending actions and terminal receipt boundary validation, preserving lifetime IDs;
- deterministic heard-state validation, monotonic alignment and reserved path protection;
- trailing boundary seals (including zero-step exports) and offline boundary replay.
- shared binding-checked surface negotiation, bounded replies and health/inactivity;
- real embedded C ABI and Linux/Apple UID-authenticated private Unix socket framing.
- pinned operation-schema selection, capability replacement and grant/resource checks;
- opt-in digest-pinned local trusted extension catalogs, single-owner routing and
  original read/result bindings (see [provisioning and qualification](trusted-extension-catalog.md));
- one-time clock translation and complete deterministic act bytes fsynced before exposure;
- durable bound-surface admission/outcomes, terminal dedup and staged inbox boundary mutations.
- lease-scoped private event routing with durable shared surface sequence/clock highwaters;
- bounded caller-scoped native reads, timeout/late-reply fencing and revocation purging.
- Linux boot-identity/BOOTTIME and Apple boot-session UUID/continuous-time clocks persisted in the WAL;
- service-backed socket and C ABI dispatch pumps, ordered by committed calls;
- automatic same-envelope reconciliation after native reconnect and WAL reopen.
- durable speech capture start/revision/end fencing and provider call deduplication.
- native runtime-issued owner heard reads with durable pre-reasoning barriers,
  conservative M-D spans and journaled runtime.heard boundaries (see [live-heard.md](live-heard.md)).

`Runtime::from_fixture` and `MockEngine` implement the data-only engine seam in
`conformance/abi/README.md`. Scripted engine usage is not a real Wasm measurement.
The mock seam remains for fixture execution. Opt-in `native-engine` connects
the shared `coglet-engine` API to transactional `StepHost` imports and the
qualified WAMR interpreter. It is a bounded, real-Wasm route, not full ABI
qualification: restricted WASI preview1 file imports use the `coglet-fs`
step overlay with `/code`, `/data` and `/logs` preopens. Forbidden imports
still fail admission; no ambient host filesystem is exposed.

## Verification

See [daemon-restart.md](daemon-restart.md) for ADR-0027 boot revalidation,
real guest-after-SIGKILL proof, the named requirement matrix, CPU-loaded stress
method and the explicit supervisor socket-cleanup / queued-pause boundaries.

See [native-engine.md](native-engine.md) for the opt-in real-Wasm scheduler,
trusted supervisor configuration and its remaining qualification limits.
Apple evidence is [run 45286035005](https://github.com/softmax-universe/coglet/actions/runs/45286035005),
source head `a73a322472c11478857a81c2aa1ab42b8b7c6d3e`: native arm64 macOS
engine/runtime default + native-engine suites and compiled echo C-ABI smoke
passed with zero ignored tests. x86_64 macOS was cross-built; arm64 iOS was
archive-built and SDK-linked (minimum 15.0), **not executed on a device**.
The evidence table distinguishes this snapshot from final PR-head checks.

```sh
cargo test -p coglet-runtime
cargo clippy -p coglet-runtime --all-targets -- -D warnings
cargo fmt -p coglet-runtime -- --check
python3 crates/coglet-runtime/tools/check-primary-abi.py
python3 crates/coglet-runtime/tools/check-surface.py
```

The launcher runs the unmodified `conformance/abi/cases.json` corpus through the
official black-box harness and actual `cogletd --mock-stdio` executable: all 21
primary cases pass, with no skipped cases. It checks the fresh JUnit case names
and counts; this is a **subset gate, not full ABI conformance**. The executable
does not read case data or expected responses; it computes records through the
runtime and mock engine. Each step emits the full result and then a hash-only
frame because the harness's separate `journal_hash` assertion consumes a frame.

Rust integration tests also compare complete journals and hashes. The replay
test supplies the same independent mock program twice, drives
it with recorded inputs, compares every step, and checks that a changed program
fails. No recorded output files or return values are fed into guest execution.

## Baseline milestone and follow-up boundaries

The primary ABI baseline does not qualify full ABI. Operational Linux surface
qualification is documented separately in [qualification.md](qualification.md),
with exact native test names, ownership and supported bounds. The small frozen
negotiation corpus alone is not operational evidence.
`every_primary_abi_case_is_executed` also independently replays
all 21 primary cases and compares every per-step state hash.

Remaining qualification is owned serially by board cards:
- `t_01a2085c6e546405`: operational surface, production daemon and durable outbox
  (implemented; independent review/CI/landing recorded on PR #53).
- `t_01a2085cca051179`: full ABI profiles, engine recovery and trace import/export.

The unimplemented boundaries remain explicit:

- All frozen ABI wire profile adapters are exercised by
  `python3 crates/coglet-runtime/tools/check-abi.py`: 128 cases, zero failures/skips.
  This is aggregate profile coverage, not one engine executing every program:
  runtime scripted steps and archive checks, actual WAMR module admission, and
  the SDK/shared native mock reference-guest route. The launcher builds the
  `native-engine` feature; feature-free binaries refuse module admission. The
  admission-only fixture mode explicitly maps `fixture.interpreter` to the pinned
  WAMR interpreter; unsupported modes fail before instantiation. Manifest admission
  uses the supplied trusted catalog and authorization snapshot, not transport
  authentication or live lease freshness. Archive view selection is an explicit
  process argument (`--trace-view state|validity`), never a case-name lookup.
  Heard archive fixture setup is entirely supplied in each trace; registry labels
  require no live provider. Recursive inventory and fresh JUnit checks fail closed
  on new/unrouted files, failures or skips. All-profile independent replay remains
  a separate gate; offline archive reconstruction does not satisfy it.
- Native orchestration now attaches the durable service explicitly in both transports.
  The default probe modes remain negotiation-only. Linux native clock continuity uses
  BOOTTIME (including suspension/downtime), not per-link elapsed time or wall-clock
  guesses. A changed boot identity, old unanchored dispatch ledger or poisoned WAL
  fails closed. macOS uses `kern.bootsessionuuid`; iOS uses a race-safe OS-entropy
  process epoch ([ADR-0039](../../docs/adr/0039-ios-process-runtime-clock.md)).
  Both use checked `mach_continuous_time`. iOS anchored WALs refuse cross-process
  reopen before engine admission; same-process reopen retains elapsed time.
  Entropy/clock failure has no fallback. This is not physical-device qualification.
  See [Apple evidence](native-engine.md#apple-platform-evidence) for execution vs link scope.
  Dispatch order is retained/checked against initial intent order and committed action
  arrays, not sorted IDs. Expired undispatched intents receive durable local rejections.
- Operational private events and reads use the shared library session attached
  by both service-backed native endpoints. Private event admission intersects
  manifest subscriptions with current grants/resources; grant replacement, expiry,
  disconnect and restart purge old-lease queued input while retaining sequence
  highwaters. Capture identities are never reopened across epochs/reconnects;
  partial/end revisions must increase, and late/equal revisions are counted without
  forwarding private text or queueing a wake. Source highwater, revision/end fence
  and enqueue are one WAL transaction. Provider tool calls require a retained
  successful owner session and identical repeated calls do not queue another event.
  Tombstones are non-evicting and capped at 4096 utterances plus 4096 provider calls
  per service; exhaustion fails closed, requiring explicit supervisor lifecycle
  reconciliation, not silent eviction/reuse. This is a bounded qualification route,
  not an indefinitely running speech service.
  Native reads never invent guest events; pending+completed observations are bounded
  to 128 with 4096 non-evicting correlation tombstones per connection. Stale durable
  epochs purge read replies/results. Heard reads require a retained accepted speaker
  item and current speaker lease; provider-native items bind conservatively to an
  unambiguous retained live session (see live-heard.md for lifecycle restrictions).
  Both native transports initiate heard reads from authenticated
  position/barge observations when speech.history is advertised. An atomic durable
  barrier prevents reasoning until the corresponding runtime.heard boundary is
  committed; revoke, expiry, disconnect and restart purge abandoned barriers with
  private input. This is a runtime policy, not a new guest import or surface RPC.
- Close/disconnect with real device I/O and complete speech temporal normalization
  remain unqualified. The native operational test backend is a deterministic receipt
  ledger, not a physical device/provider or production surface SDK qualification.
- Opt-in native scheduling is operational over socket and C ABI, with real Wasm
  tests for commit-before-dispatch call order, trap rollback, one-time batch
  consumption and byte-identical reconciliation after WAL reopen. The default
  feature-free service mode only hosts the outbox. Native execution currently
  refuses all WASI imports; full transactional WASI is the serial ABI qualification
  gate. No arguments still prints the version and exits.
- Trace headers/blob import/export and offline archive validation now cover all
  frozen boundary/receipt, provenance and heard-archive vectors in the Rust target
  `complete_frozen_archive_profiles_validate_without_guest_execution`. Import
  reconstructs host calls/pending state, checks mount-constrained recorded guest
  deltas and recomputes hashes. It is **not** independent guest re-execution,
  signature verification or proof of a producer's cutoff/completeness. Use
  `Runtime::replay` with an independently supplied program for re-execution.
  `RuntimeService::export_trace` includes reopened journal history and seals any
  staged applied boundary durably before export; imports retain all verified blobs.
  `RuntimeService::start_segment` now atomically transfers the complete coordinator
  state in the same exclusively locked WAL. Retained segment cutoffs and immutable
  headers bind predecessor seals to successor host state; old journals and dispatch
  receipts are never cleared. `export_segment` retains sealed predecessor exports.
  `durable_segment_transfer_retains_history_router_and_trace_local_chain` exercises
  three segments, reopen, trace-local counters, pending timer delivery, old exports,
  and forged headers/cutoffs/clock/source-floor rejection. Native scheduling must be
  re-enabled explicitly with the successor trace identity.
- Admission and attempt errors are distinct. `Engine::admit` cannot run guest code;
  native load/instantiate happens there, with one unchanged deadline through entry.
  Known guest failures return observed usage and produce ordinary aborted records.
  Before execute, the coordinator durably reserves the exact inputs. Unknown engine
  errors, malformed reports and uncertain commits retain a non-retryable fence across
  reopen. No unknown usage is fabricated. `recover_attempt` accepts a complete,
  caller-authenticated original worker observation, validates its exact inputs and
  host transitions, and atomically commits it without guest execution. It is not
  authentication or independent guest replay. If no observation survives (including
  a crash between durable reservation and guest entry), the guest remains fenced;
  there is deliberately no unsafe "clear and retry" API. Exports/seals/segmentation
  refuse unresolved attempts. Later terminal appends backpressure until recovery.
  Private leases are revoked on restart, but the already-attempted queue prefix is
  retained solely for reconciliation, never fresh guest delivery.
- Recovery reconstructs files, lifetime IDs, pending effects and inbox wakes from
  the configured immutable root and each segment's recorded host transitions.
  Router checks include capacity, reserved ordinal order, schemas, exact pending
  inbox-wake completeness, timer bindings and source reservation uniqueness. Fresh
  services atomically seed all initial inbox wakes. Private leases bind ordinal,
  exact event, source, epoch, nonzero expiry and source highwaters before cleanup.
  Recovery verifies retained blob digests and reruns no-guest attempt preflight on
  a disposable clone, including effective budgets, event/boundary semantics and
  engine/grants/seed shapes. Own-source reservations are derived from the whole
  multi-segment WAL, router and host-appended inbox: duplicate reuse and generated
  wake gaps fail closed; authenticated boundary inputs may advance their source
  floor. Clock continuity retains cross-segment lower-bound checks. Header-less old WALs fail
  closed; there is no silent migration. The WAL has a 64 MiB state-frame bound and
  no reclamation yet; segment headers/history consume storage and can eventually
  backpressure, rather than evict lifetime data or receipts.
- `seal_boundary` atomically persists the seal, inbox append/pending removal and
  newly queued inbox wakes; heard-only seals queue no wake. Applied staged changes
  enter the next real step or a trailing seal, including zero-step exports.
- Native pump liveness while fenced retains existing IDs but refuses new dispatches
  and terminal appends. Typed GuestExit (including zero) maps to guest_return;
  native WASI is still refused, so this is not a proc_exit execution proof.
  See [ABI applicability and remaining gates](abi-qualification.md). Real WAMR
  two-step journal replay now compares state hashes with independent execution:
  wall duration observations may differ, but both observed limits/status and all
  deterministic record fields remain checked. Simple enqueue retains bounded
  tail admission while fenced; it cannot consume or reorder the attempted prefix.
- Independent exact-head review, green CI, merge, and board review handoff.
  Segment transfer also retains pending actions, LLM requests, inbox wakes and
  durable dispatch receipts across successor reopen. Extra malformed
  header/record/blob and divergent-program replay refusals are named tests.

The internal WAL format is not the exported ABI journal. Each internal commit
contains runtime host state and an optional ABI record in one checked frame;
`DurableStore::export_journal` emits only u32be-framed deterministic CBOR ABI
records. Complete corrupt frames fail closed; only incomplete trailing bytes are
truncated. Silently rolling back a complete corrupt commit could re-dispatch an
already executed external effect and is not an acceptable recovery strategy.

The supervisor must supply a private persistent directory for WAL storage.
Public coordinator event admission assumes prior surface authorization; reserved
runtime/inbox events are refused at that external seam.

`Runtime::from_fixture` is test setup, not a production archive admission API.
The primary `abi.step` setup has no `initial_actions` field and therefore defaults
it to empty; future trace import must require the complete header explicitly.
The shared source sequence persists in service state and advances for committed
runtime-source events and receipt appends. Fresh fixture setup starts at zero;
production restart/segment transfer must retain the full lifetime source metadata.

## Native surface boundary

`tests/operational_surface.rs` separately exercises the library authority and WAL
seams (not native operational qualification). Named tests cover byte-identical
retry after clock advance, reconnect recovery without a new action, authenticated
source/admission binding, one terminal append across restart, zero-step trailing
export, persistent grant-epoch fencing and deadline-relative retention. Native
transport evidence and the distinction between runtime pending work and actual
device-I/O shutdown are explicit in [qualification.md](qualification.md).

`tests/native_operational.rs` exercises the actual C pointer boundary and an actual
`cogletd` Unix-socket subprocess. Both paths check reverse-lexical invocation order,
byte-identical automatic retries after clock advance and WAL reopen without fresh
grants, accepted-action health counts, authenticated outcomes and duplicate terminal
delivery, and kill. Reopened WALs contain exactly one terminal mutation/wake per
action. Separate regressions inject a real OS WAL-write failure and check clock and
retained-dispatch fencing, and reject an expired outbox head without starving later
actions. Two further actual native-path tests send start/partial/end and stale
revisions, read back durable end fences/discard counts, and prove disconnected
private input is not revived. Library tests prove source-order guest delivery and
that router-full does not commit a revision/highwater. Playback tests additionally
cover item/stream owner/epoch/chunk bindings, exact barge pairing, terminal freeze,
provider-native item ownership and durable recovery (see live-heard.md for the
conservative provider lifecycle restrictions). Heard regressions cover
native socket/C reads, conservative prefix construction, revoked late replies and
runtime-clock barrier expiry. With native-engine, those same native paths execute
real Wasm and verify runtime.heard precedes each position-consuming step.

`cargo build -p coglet-runtime --lib` produces static and shared libraries.
`include/coglet_runtime.h` exposes the frozen coglet_surface_open/send/recv/close
signatures plus runtime-owned configure/start/stop. Before open,
trusted application code configures UTF-8 JSON with `surface` and
`permitted_coglets`. Configuration is immutable per process, identical retries
are idempotent, and hello supplies no authentication. Handle calls must be
serialized and recv polled at least once per second on the non-audio control
queue. No calls after close. Unsupported operations cannot acquire authority.

To attach the durable service, call `coglet_runtime_start` with UTF-8 JSON
`{"wal":"/absolute/private/state.wal","setup":{...}}` before opening links.
`setup` is the current runtime configuration shape (code/manifest/files/pending
ledgers); this is not yet a production signed archive admission API. Stop returns
would-block while any service-backed link remains live; close links before restart.
Without start, open remains the existing negotiation probe. Linux and Apple clocks
support supervised start; [Apple evidence](native-engine.md#apple-platform-evidence)
distinguishes native macOS execution from iOS archive/link only. Startup JSON has a
separate 16 MiB bound because it carries hex-encoded Wasm; surface frame limits do
not change. The supervisor remains trusted and signed archive admission is open.

The equivalent Linux daemon transport is:
`cogletd --surface-service-socket SOCKET BINDING_JSON PEER_UID WAL SETUP_JSON`.
This mode serves the coordinator and optionally the qualified native-engine scheduler
described above. Supply a supervisor-owned persistent WAL directory.

On Linux, `cogletd --surface-probe-socket ABSOLUTE_PATH BINDING_JSON PEER_UID`
serves the same negotiation shell. The parent directory must be owned mode0700
(or is created mode0700), the socket is0600, and SO_PEERCRED must match the
supervisor-configured peer UID. Ancestors must be supervisor-trusted; existing
socket paths are never unlinked. This mode is NOT the production runtime daemon.

`check-surface.py` stages unchanged original surface cases whose peer_role is
surface (the harness plays a surface, runtime is under test). **All current
runtime-side cases are negotiation-only**: two cases over each real transport,
four passes with zero skips. It verifies the fresh JUnit inventory, and compiles
and runs `tests/native_surface.c` against the actual shared library. Native probes
cover null/unsupported open, copied input lifetime, size query, short-buffer
nondequeue, exact read, would-block/n=0, bounded-send no-effect, malformed input,
close, unsolicited heartbeat and stale-link timeout. Rust tests cover hostile
binding/lifecycle/profile, framing, UID denial, path permissions and idle socket
heartbeats. These tests alone do not qualify grant/private-event/dispatch
semantics; the separate operational/native targets supply that evidence.
