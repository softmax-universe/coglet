# coglet

Sandboxed Wasm coglets, the adapters that host them, and the router that controls them.

- Cogent design: [docs/cogent-design/README.md](docs/cogent-design/README.md), one of the core design documents: what a cogent, a coglet, an adapter, a runtime, a router and a hub are, and how they fit together
- Plan: [docs/plan.md](docs/plan.md)
- Design: [docs/design/coglet-runtime.md](docs/design/coglet-runtime.md)
- The previous COG/LET Python framework lives on branch `legacy/v0` (tag `legacy-v0`).

## `coglet` CLI (sandbox, enrollment and local operator control)

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
  deploy    Build/sign, transfer, install and start through the router's local admin socket
  push      Build/sign and transfer only; does not install or start
  status    Obtain a durable runtime status observation (fresh ID for a fresh sample)
  stop      Stop at a confirmed runtime boundary
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
`wasm32-wasip1` is accepted for compilation. Native runtime admission hosts the
restricted WASI preview1 file preopens (`/code`, `/data`, `/logs`) and still
refuses clock, random, poll, symlink-create and hard-link imports.

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

`deploy`, `push`, `status` and `stop` use the [ADR-0018 local operator
socket](docs/adr/0018-local-operator-control.md). Run the CLI **on the router host
as its service user**, directly or by launching that CLI over SSH. Never forward
the socket. The client checks the service-owned 0700 parent, 0600 socket and
OS-authenticated server UID; there is no root exception or new iroh method.

```sh
# Persist a fresh 32-hex operation ID BEFORE submission. Keep it for retries.
# The router must already have this source, recipe, delegation and actor grants provisioned.
coglet deploy --operator-socket "$ROUTER_STATE/operator.sock" \
  --operation-id "$DEPLOY_ID" --device "$DEVICE_ID" --target "$COGLET_ID" \
  --source-commit "$SOURCE_COMMIT" --build-profile "$BUILD_PROFILE" \
  --delegation-id "$DELEGATION_ID"
# push takes the same flags but stages verified code only: no install or start.
coglet status --operator-socket "$ROUTER_STATE/operator.sock" \
  --operation-id "$STATUS_ID" --device "$DEVICE_ID" --target "$COGLET_ID"
coglet stop --operator-socket "$ROUTER_STATE/operator.sock" \
  --operation-id "$STOP_ID" --device "$DEVICE_ID" --target "$COGLET_ID"
```

Deployment builds/signs the **router-provisioned pinned source**, not an unsigned
local sandbox package. The CLI accepts no repository URL, shell recipe or private
signing key. It does not grant itself authority or bypass policy. The router
performs build → transfer → install → start and confirms running. `push` is a
separate initial-staging operation, not an alias for deployment. The current
producer supports initial deployment only; pushing and then deploying the same
provisioned generation under a different operation ID can conflict. Hot updates
and ADR-0031 filesystem/manage conversion are separate producer work.

A validated response is printed as JSON, including failed/queued receipts.
Exit **0** means `applied`; **3** means durable `queued` admission (NOT success);
**1** means rejection, failed/cancelled receipt, or transport/validation failure.
Clap usage errors exit 2. After a timeout, disconnect or malformed response,
outcome may be unknown: retry the **same operation ID and identical intent**,
never mint another ID to repeat uncertain effects. `--timeout-ms` (1..300000,
default 30000) bounds waiting, not application authority. There is no automatic
retry. Status retries return the original observation: use a fresh ID for a new
sample. Deploy is a saga, not rollback: failure may leave prior stages applied.

### Named CLI checks

```sh
# Integration tests need the real sibling cogletd and router build validator.
cargo build --locked -p coglet-cli -p coglet-runtime -p coglet-hub
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
Additional named checks:
`cli_reference_router_deploy_push_status_stop_and_idempotent_retry` (CLI process,
in-process reference router, real signed builder and encrypted iroh to production
DeviceHandler), `cli_reference_router_push_is_staging_not_install_or_start`,
`cli_operator_queued_failed_and_malformed_never_report_success`,
`operator_client_refuses_symlink_and_public_socket_directory`, and
`operator_response_rejects_ambiguous_or_unbound_receipts`.
Synthetic fault peers are only refusal controls, never deployment proof. The CLI
integration uses an in-process runtime handler; actual daemon/restart qualification
is retained in the producer's `operator_cogletd_deploy_running_stop_status` proof.
These are CLI acceptance checks, not full router or M1 conformance.
