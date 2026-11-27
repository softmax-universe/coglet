# Live coglet inspector

Read-only web monitor served locally by an **opt-in producer observer**, not a
public dashboard. No frontend build, CDN, external font or Node service.

## Watch a running reference runtime

```sh
cd tools/visualizer
uv sync --locked
# Have your reference driver use this command in place of cogletd --mock-stdio:
uv run observe.py --records /private/run/journal.jsonl --cogletd /path/to/cogletd
# In another terminal, before or after the driver starts:
python3 server.py --records /private/run/journal.jsonl --world-path /data/world.json
```

The driver continues to send the existing four-byte-length-prefixed CBOR ABI
requests and read the two documented responses per step. The observer launches
actual `cogletd --mock-stdio`, forwards bytes unchanged and exports only its real
`journal` replies. It creates a **new mode-0600 file** and refuses overwrites.
This is a reference-engine run, not WAMR execution. It does not inject expected
outputs, change the production capability catalog or add a wire RPC.

Open the private `http://127.0.0.1:8088/#…` launch URL printed by the server. The
fragment capability is consumed in memory and removed from browser history;
API calls carry it in an authorization header. Reload by reopening the original
launch URL. Restarting the server revokes the old capability. Do not share it.
There is deliberately no wildcard/listen-network option or runtime mutation route.
The authenticated POST endpoint computes offline simulations only; it does not
write an archive, control a runtime, or contact a provider.

## Observe an authorized native or simulator export

```sh
python3 tools/visualizer/server.py --trace /private/run/trace.json
```

`--trace` reads `records` from an operator-selected ABI trace JSON export, e.g.
`RuntimeService::export_trace`. The producer must authorize release of its
private event/inbox/data bytes, export at committed boundaries and publish the
complete JSON atomically (write a private sibling then rename). The viewer does
not open live runtime WALs, hub SQLite databases or keys. A trace is **not**
authenticated or independently replayed by displaying it. Use the runtime's
verifier/replay API for those claims. `--records` instead tails newline-terminated
JSONL records; an incomplete final append is never shown. Both paths are checked
again on each poll; missing/invalid input clears displayed private state.

Latest 128 records are retained in the view. Inputs are capped at 16 MiB;
JSONL records at 1 MiB. Rotate to a new explicit export/server before the cap.
Historical rows are selected by coglet, trace and step; **Follow live** returns to
the newest exported record. Observed surface sources are event provenance, not
a surface registry or a health probe. Missing metadata stays unknown. Export
mtime is shown separately from runtime readiness; an unchanged export never
claims a live/healthy runtime. Polling is every 500 ms and opt-in by opening this
local viewer. Exports may contain sensitive data; the containing directory must
also be private. `/code`, `/logs`, input files and trace blobs are never sent to
the browser. `/data` and event payloads are explicitly selected for display.

### World observation adapters

`--world-path` explicitly selects a UTF-8 JSON `/data` file in the exported
snapshot. These are local **presentation adapters**, not new capability schemas,
physics models, or a claim of running the M2 simulator:

```json
{"kind":"gridbot","width":8,"height":6,"robot":{"x":2,"y":3},"goal":{"x":6,"y":1},"walls":[[4,2]]}
```

```json
{"kind":"cartpole","x":0.25,"theta":0.15}
```

Grid coordinates are zero-based, y increases downward; dimensions are integers
1–128. Robot and optional goal must be in bounds; walls are coordinate pairs.
Cartpole theta is radians from upright, positive toward screen-right; finite x
is scaled at 70 pixels/unit and clipped visibly to track bounds. Unsupported or
malformed observations show an explicit unavailable state, never a made-up world.
Simulators with a different state shape should supply a **trusted local export
adapter** rather than change their protocol to match this UI.

## Offline simulated world mode

The additional panel runs the **production Python M2 world model**, not a second
JavaScript approximation and not recorded-output injection. It reconstructs the
world from an explicit seed/configuration and ordered action inputs. Every
baseline world state/hash must match a recorded `sim.gridbot_state` or
`sim.cartpole_state` event before a result is shown. An edited action forks that
same baseline and reports the first divergent **world** step and the computed
versus recorded world hash. The world-step slider supports Home/End and arrow
keys; previous/next buttons also work. Replay recorded inputs resets the fork.

For the committed actual M2 Gridbot run recorded against the current production
world model (producer/input provenance in
`tests/fixtures/world-m2-gridbot-trace.provenance.json`):

```sh
cd tools/visualizer
uv run --locked prepare_simulation.py \
  --trace tests/fixtures/world-m2-gridbot-trace.json \
  --world gridbot-v1 --seed 42 --config '{"width":8,"height":8,"max_steps":16}' \
  --actions '["east","east","south","south"]' \
  --output /private/run/world-replay.json
uv run --locked server.py --trace tests/fixtures/world-m2-gridbot-trace.json
```

Gridbot's `max_steps` is part of its production world snapshot hash, so it is a
required configuration pin. Both Gridbot display fixtures were recaptured by
`regenerate_state_hash_fixtures.py` for ADR-0035 with `max_steps=16`; mismatched
configurations are refused, not adapted by rewriting recorded hashes. The
generator runs the actual CLI and Gridbot WAMR producers and independent replay;
provenance records the source revision, complete trace/proof digests and omitted
private fields. World-state hashes retain their separate existing definition.

Open the private launch URL and choose `world-replay.json` under **Load simulation
archive**. No upload is made until that explicit selection; the archive stays
in the browser/server process memory and is not persisted. **Follow live** leaves
simulation, discards the imported archive and resumes the original export.
Invalid, oversized, incomplete or mismatching imports clear previous results.
The preparation command creates a new private mode-0600 file atomically and
refuses overwrite; the containing directory must already exist and be private.

The viewer-local envelope (not a new ABI schema) is:

```json
{"format":"coglet-world-replay-v1","world":"gridbot-v1","seed":42,
 "config":{"width":8,"height":8,"max_steps":16},"actions":["east","east","south","south"],
 "records":["actual ABI display records with matching world observations"]}
```

The displayed placeholder in `records` above must be replaced with the actual
record objects; the preparation tool does that without modifying them. Cartpole
uses `world: cartpole-v1`, explicit `config: {"max_steps":16}` and `left`/`right`
actions. Supply the producer's actual seed, configuration and actions, never
infer missing input pins from displayed output. The real M2 fixture's producer
records and provenance are unchanged. Limits: 16 MiB input, 1,000 world actions,
4,000 records, complete one-observation-per-world-step history, one coglet/trace.
A fork preserves the recorded action count; invalid or early-terminated action
sequences fail closed rather than inventing continuation states.

**Qualification boundary:** this proves seeded **world dynamics** only. It does
not re-execute Wasm, verify guest `output_hash` values, rerun the coglet policy,
authenticate producer identity or replace a complete private ABI archive.
The guest timeline/files remain explicitly recorded observations and do not
change when world inputs are edited. For complete canonical archives, the
separate [native replay panel](native/README.md) re-executes the guest and forks
successful action receipt result inputs, comparing actual native tree hashes.
Do not present the world-model panel alone as native verification.

Named browser acceptance:
`test_recorded_m2_upload_scrub_step_replay_same_final_hash_and_edited_input_divergence`
loads the actual M2 run, keyboard-scrubs and steps, independently reconstructs all
recorded world hashes, shows a first divergent edited action, rejects malformed
and oversized input without stale state, and checks desktop/390px layouts.
The existing Python CI job discovers it without any workflow edit.

## Tests / proof scope

```sh
cd tools/visualizer
uv sync --locked
uv run playwright install chromium
COGLETD=/absolute/path/to/cogletd uv run pytest
# Optional real-render screenshots + JUnit:
COGLETD=/absolute/path/to/cogletd VISUALIZER_PROOF_DIR="$PWD/proof" \
  uv run pytest --junitxml=proof/visualizer.xml
```

CI downloads the actual immutable `v0.0.0-rc1` Linux x86_64 release, verifies its
pinned SHA-256 and runs headless Chromium. No network stub or expected journal
output is substituted for the binary. Named browser cases:

- `test_live_cogletd_events_and_state_render`
- `test_world_observation_and_history_are_live[world0/world1]`
- `test_filter_keyboard_error_and_recovery`
- `test_real_inbox_boundary_and_surface_source_are_inspectable`
- `test_surface_observation_and_untrusted_text_render_as_text`
- `test_native_cli_wamr_trace_events_state_hash_and_history`
- `test_native_m2_gridbot_data_state_and_history`

The first five drive actual rc1 ABI steps through `observe.py`, assert produced
record identity/events/files/hash, live history arrival, both world renderers,
genuine inbox boundary effects, escaped untrusted text, keyboard interactions,
failure and recovery, and 390px overflow checks when collecting screenshots. The
world inputs are labeled reference scenarios written by the scripted engine;
**they do not qualify M2 world dynamics, WAMR, M1 or a real hub debug client**.

`test_native_cli_wamr_trace_events_state_hash_and_history` loads a records-only
`--trace` export from an actual child-cogletd WAMR sandbox run (`coglet run`
from PR https://github.com/softmax-universe/coglet/pull/82 at
`866ede9b1ee7a1c20cc6e4afed723bfcb093ee4d`). It asserts two committed steps,
`runtime.debug_step` events, distinct state hashes, history selection, and that
`/code` is not shown. Provenance pins the producer SHA and omits
`initial_files`/blobs. Its projected `/data/inbox.jsonl` is empty, so it is not
the state-rendering proof. This is **not** live hub debug and not independent
replay verification. rc1 has no `--sandbox-stdio`, so CI does not rebuild the
producer; it re-reads the pinned actual records.

`test_native_m2_gridbot_data_state_and_history` loads a display trace from an
actual child-cogletd WAMR Gridbot episode of reviewed PR
https://github.com/softmax-universe/coglet/pull/83 at
`dd98c4239d505401c9403213b9efd931d392234f`. The producer proof independently
replayed nine guest steps (four actions, goal reached). The committed file
copies `/data` hex, events, engine and step hashes verbatim from that WAL
export. The full records file is 44,275,377 bytes because every step repeats
the 1.2 MiB guest, which is above this viewer's 16 MiB cap, so `/code`,
`input_files` and blobs are omitted from the fixture and named in its
provenance. The browser test asserts the exact decoded `/data/inbox.jsonl`
receipts on the latest step and on an earlier step, the stable `/data/count`
byte, history selection, and that `/code` is absent. Receipt clocks are the
producer clock, so a second episode is not byte-identical. This does not drive
the world canvas and is not a hub debug client.

Unit/HTTP tests cover partial writes, atomic trace replacement, bounds, unsafe
files, malformed records, bearer authorization, origin/Host checks, and headers.
The observer test checks byte-preserving replies, actual journal export, private
file mode and overwrite refusal.

## Remaining producer boundary

Merged `coglet-hub` exposes operator `Kernel::status` metadata but no separately
authenticated external debug client/relay. Frozen `hub-protocol.md` requires
explicit exact grants and actor/ticket provenance for remote reads and forbids
default state export. This viewer does **not** bypass that boundary or claim a
production hub connection. Native CLI/M1/M2 producers can supply authorized
exports through the `--trace` seam. The CLI WAMR case above is pinned to PR82
`866ede9b1ee7a1c20cc6e4afed723bfcb093ee4d` and does not render nonempty native
`/data`. The Gridbot case renders verbatim `/data` from reviewed PR83
`dd98c4239d505401c9403213b9efd931d392234f`. Neither invents a hub debug
transport. Full card acceptance still needs required CI, independent exact-head
review, and merge.
