# Runtime core (implementation in progress)

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
- real embedded C ABI and Linux UID-authenticated private Unix socket framing.
- pinned operation-schema selection, capability replacement and grant/resource checks;
- one-time clock translation and complete deterministic act bytes fsynced before exposure;
- durable bound-surface admission/outcomes, terminal dedup and staged inbox boundary mutations.
- lease-scoped private event routing with durable shared surface sequence/clock highwaters;
- bounded caller-scoped native reads, timeout/late-reply fencing and revocation purging.

`Runtime::from_fixture` and `MockEngine` implement the data-only engine seam in
`conformance/abi/README.md`. Scripted engine usage is not a real Wasm measurement.
The crate currently owns its `Engine` and snapshot seams because the shared
`coglet-engine` and `coglet-fs` APIs were empty at implementation start. Those
seams must be integrated when their owning lanes publish the real APIs.

## Verification

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

This component-level baseline intentionally does not qualify full ABI or operational
surface behavior. `every_primary_abi_case_is_executed` also independently replays
all 21 primary cases and compares every per-step state hash.

Remaining qualification is owned serially by board cards:
- `t_01a2085c6e546405`: operational surface, production daemon and durable outbox.
- `t_01a2085cca051179`: full ABI profiles, engine recovery and trace import/export.

The unimplemented boundaries remain explicit:

- Full ABI profile adapters: module/manifest admission, trace/provenance validation,
  terminal-receipt and heard archive corpus import/validation (the runtime seams
  exist, but those language-neutral profiles have not been implemented).
- Native endpoint dispatch orchestration: the library has a durable translated outbox
  and bound-source receipt materialization, but transports still create probe-only sessions.
  Both native endpoint modes must attach the durable service, dispatch committed actions
  in call order, reconcile retained IDs on reconnect and use one runtime clock epoch.
- Operational private events and reads exist in the shared library session, but
  native endpoints still need to attach it. Private event admission intersects
  manifest subscriptions with current grants/resources; grant replacement, expiry,
  disconnect and restart purge old-lease queued input while retaining sequence
  highwaters. Provider tool calls require a retained successful owner session.
  Native reads never invent guest events; pending+completed observations are bounded
  to 128 with 4096 non-evicting correlation tombstones per connection. Stale durable
  epochs purge read replies/results. Heard reads require a retained accepted speaker
  item and current speaker lease; provider-native items without retained ownership
  currently fail closed. Heard-state construction from reads remains to be wired.
- Close/disconnect with pending real native I/O: native pointer/buffer/backpressure
  and heartbeat probes exist, but no operational native I/O can be admitted yet.
- `cogletd` production server mode. Only --version, --mock-stdio and the explicit
  --surface-probe-socket negotiation shell exist. No arguments prints the version
  and exits (workspace packaging smoke contract); unsupported arguments fail closed.
- Engine admission/attempt-error contract and full recovery-invariant validation.
  Recovery checks all code files, pending-ledger membership, inbox wake indices,
  trace identity and journal head/chain, but not a full reconstruction of all host
  metadata from prior commits yet. Router metadata also needs semantic validation.
- Trace header/blob import/export and host-state transfer to a new segment after
  sealing. `seal_boundary` forbids later steps in that segment. The coordinator
  atomically persists the seal, inbox append/pending removal and newly queued
  inbox wakes; heard-only seals queue no wake. Terminal surface receipt/outcome methods
  now persist dedup with exact staged inbox bytes, pending removal and one wake. Staged
  changes enter the next real step or a trailing seal, including zero-step exports.
- Independent exact-head review, green CI, merge, and board review handoff.

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

## Native surface boundary (negotiation gate only)

`tests/operational_surface.rs` separately exercises the library authority and WAL
seams (not native operational qualification). Named tests cover byte-identical
retry after clock advance, reconnect recovery without a new action, authenticated
source/admission binding, one terminal append across restart, zero-step trailing
export, persistent grant-epoch fencing and deadline-relative retention. Production
clock/lifecycle wiring, full private-event temporal normalization, transport dispatch and native
pending-I/O shutdown remain required before this card is complete.

`cargo build -p coglet-runtime --lib` produces static and shared libraries.
`include/coglet_runtime.h` exposes the frozen coglet_surface_open/send/recv/close
signatures plus runtime-owned `coglet_runtime_configure(config,n)`. Before open,
trusted application code configures UTF-8 JSON with `surface` and
`permitted_coglets`. Configuration is immutable per process, identical retries
are idempotent, and hello supplies no authentication. Handle calls must be
serialized and recv polled at least once per second on the non-audio control
queue. No calls after close. Unsupported operations cannot acquire authority.

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
heartbeats. These tests do not qualify unimplemented grant/private-event/dispatch
semantics merely because the present two-case runtime-side corpus passes.
