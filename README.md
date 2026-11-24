# coglet

Sandboxed Wasm coglets, the adapters that host them, and the router that controls them.

- Cogent design: [docs/cogent-design/README.md](docs/cogent-design/README.md), one of the core design documents: what a cogent, a coglet, an adapter and a runtime are, and how they fit together
- Plan: [docs/plan.md](docs/plan.md)
- Design: [docs/design/coglet-runtime.md](docs/design/coglet-runtime.md)
- The previous COG/LET Python framework lives on branch `legacy/v0` (tag `legacy-v0`).

## `coglet` sandbox CLI (M2 component, not full deployment acceptance)

Build both executables together (WAMR requires CMake and a C compiler):

```sh
cargo build --locked -p coglet-cli -p coglet-runtime --features coglet-runtime/native-engine
./target/debug/coglet --help
```

`coglet --help` exposes:

```text
Usage: coglet <COMMAND>

Commands:
  build     Validate and R3-rewrite a Wasm + manifest into a new unsigned package
  run       Run explicit event batches in a child cogletd; save a private frozen trace
  replay    Independently re-execute a journal with package-pinned real Wasm
  inspect   Validate a trace and print only run metadata, not private files or events
  list      List metadata for *.json frozen traces in a local directory (sorted by name)
  identity  Create a new private device key; print its public node ID
  enroll    Enroll this device with a pinned hub; invite is read from a private file
  deploy    Unavailable until the hub operator/device Git+lifecycle adapter is qualified
  help      Print this message or the help of the given subcommand(s)
```

Use `<command> --help` for flags. Successful commands return one JSON value on
stdout; errors go to stderr with nonzero exit status. `run` reports the final
journal status (`committed` or `aborted`); an archived guest abort is a completed
sandbox attempt, not a successful guest result.

### Build → run → replay

With your own frozen manifest, bounded ABI Wasm, actual source commit, and local
recipe/policy files (all paths below are operator inputs):

```sh
./target/debug/coglet build --wasm step.wasm --manifest manifest.json \
  --source-commit "$(git rev-parse HEAD)" --recipe build-recipe.txt \
  --policy sandbox-policy.txt --output package
./target/debug/coglet run package --events batches.json --output runs/first.json
./target/debug/coglet replay runs/first.json --package package
./target/debug/coglet inspect runs/first.json
./target/debug/coglet list runs
```

Create `runs/` first. Build output contains `step.wasm`, `manifest.json` and
`provenance.cbor`; it is **unsigned local sandbox material, not a router publication**.
The supplied source commit/recipe/policy are operator assertions, not authenticated
source attestation. Their hashes, original/rewritten code hashes and the actual
R3 transform hash are recorded. The real engine independently checks ABI, R3,
compute mode, memory maxima and import policy before accepting a package.
`--cargo-manifest path/Cargo.toml` additionally runs a **trusted local** Cargo
build (`build.rs`/proc macros are not sandboxed); `--wasm` must be an artifact
reported by that exact invocation, not an unrelated stale file. `--target`
defaults to `wasm32-unknown-unknown`; install that rustup target first. Optional
`wasm32-wasip1` is accepted for compilation, but current native runtime admission
**refuses WASI imports**. Not every echo/counter guest is runnable in this slice.

`batches.json` is an array of 1..1024 nonempty arrays of frozen common `event`
objects. Each event has `source`, `kind`, `seq`, `logical_time`, `correlation_id`
and `payload`. A local explicit `runtime.debug_step` wake may use the manifest's
coglet ID as source, a unique source sequence and `{}` payload; no empty guest
batch is invented. Other events must be subscribed and explicitly granted.
Timer/inbox/outcome/provider-boundary injection is refused in a fresh sandbox.
Use a real archived coordinator trace to replay those boundaries instead.

By default the sandbox has an empty catalog and zero authority. For manifests
requiring capabilities supply both `--catalog catalog.json` (the runtime's trusted
catalog array with `events`, `reads`, `actions`, `exclusive` arrays per entry) and
`--grants grants.json` (exactly `coglet`, `operations`, `resources`). Grants are
**local timeless simulation snapshots**, not authenticated network leases; extra
fields, unknown/unneeded operations/resources and duplicate names fail closed.
Sandbox effects are journaled but never sent to a device/provider.

`run` starts the actual sibling `cogletd --sandbox-stdio` child; `--cogletd PATH`
selects a different trusted executable. There is no mock fallback. The supervisor
clears inherited environment secrets, bounds request/response bytes and stderr,
and kills/reaps its child process group on failure or a five-minute total limit.
The daemon mode is a local trusted supervisor tool, not an authenticated adapter
service or new router ALPN method. It uses the existing native engine/runtime APIs.

The run file is the frozen ABI `trace` JSON diagnostic view, not a WAL or a CLI
wrapper. It contains **private** initial files, events, journal records and blobs.
Only after complete child execution, archive validation and exact input/pin
comparison does the CLI publish it using a fsynced 0600 no-clobber temporary-file
publication. Keep its parent directory private/trusted. Packages are reserved
create-new directories; ordinary publication errors clean partial directories,
but a crash can leave an incomplete package which fails closed on read. Never
share whole traces as public metadata. `inspect`/`list` print no events or files;
`list` fails visibly on corrupted JSON runs rather than silently hiding them.

Replay validates the archive, pins code and manifest to the independently
supplied package, and runs the real guest from the trace's validated initial
snapshot and recorded inputs. It compares each deterministic record and state
hash, retaining the original checksums while excluding nondeterministic observed
wall duration per the runtime contract. It never injects recorded guest outputs,
dispatches actions or contacts providers. A trace snapshot is **not** an
authenticated producer/source-completeness assertion; archive import alone is
not guest replay. A wrong package, corrupted checksum/hash or changed guest
program fails replay.

### Enrollment and the deployment boundary

```sh
./target/debug/coglet identity private/device.key
./target/debug/coglet enroll --identity private/device.key --hub "$HUB_NODE_ID" \
  --address "$HUB_IP_PORT" --invite-file private/invite
# Recover an already committed enrollment on a new connection without reusing an invite:
./target/debug/coglet enroll --identity private/device.key --hub "$HUB_NODE_ID" \
  --address "$HUB_IP_PORT"
```

Create the private parent directory first. Identity and invitation inputs must
be regular, non-symlink, current-user-owned private files (0600 on Unix). The router
operator obtains the printed public device ID and provisions its one-use invite.
Do not pass secrets on the command line or log them. `--address` is an explicit
IP:port, but iroh authentication still pins the exact router node ID and ALPN.
The reference Kernel test exercises real encrypted enrollment, reconnect/null
recovery and refusal of the consumed invitation with no secret in output/audit.

**`deploy` and its alias `push` currently fail nonzero without effects.** The frozen
`hub.git.push`, `hub.install` and `hub.start` methods are router→device, not CLI→router.
The merged reference router has no qualified operator deployment tool, and the
streaming Git/device lifecycle producer chain is incomplete. No invented reverse
RPC, direct device bypass, registration-only success or local spool is advertised
as deployment. The M2 card's end-to-end enroll/deploy acceptance remains open
until that tool is integrated and exercised against the reference router.

### Named CLI checks

```sh
# The integration test needs the actual sibling cogletd executable above.
# The source-build test uses this installed target and its own temporary Cargo target dir.
rustup target add wasm32-wasip1
cargo test --locked -p coglet-cli
cargo clippy --locked -p coglet-cli -p coglet-runtime \
  --features coglet-runtime/native-engine --all-targets -- -D warnings
cargo fmt --all -- --check
```

Tests: `cli_build_run_replay_equal_hash_list_and_inspect` (two actual WAMR log
imports; private trace; per-step hashes),
`cli_cargo_build_compiles_real_rust_guest_before_packaging`,
`cli_bad_manifest_reports_actionable_error_before_build`,
`cli_enroll_reference_hub_and_recover_without_reusing_invite`,
`cli_help_identity_and_deploy_fail_closed`,
`sandbox_missing_grants_unknown_events_and_bad_wasm_are_rejected`,
`child_supervisor_times_out_and_bounds_both_output_streams`, and
`file_reader_refuses_fifo_symlink_and_world_readable_secrets`.
These are component tests, **not full router or M1 conformance**. Deployment, final
independent exact-head review, required CI and merge remain the card's gates.
