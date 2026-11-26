# Opt-in native scheduler (bounded qualification)

Build on the guarded Linux/macOS targets with CMake available (iOS is SDK link-only):

```sh
cargo build -p coglet-runtime --features native-engine --locked
cargo test -p coglet-runtime --features native-engine --locked
```

The existing `--surface-service-socket SOCKET BINDING_JSON UID WAL SETUP_JSON`
mode enables guest scheduling when SETUP_JSON contains a `native_schedule` object:

```json
{"trace_id":"11111111111111111111111111111111","seed":7,"runtime_bytes":8388608,"stack_bytes":65536}
```

For C embedding, pass the same optional `native_schedule` alongside `wal` and
`setup` to `coglet_runtime_start`. These are trusted supervisor inputs, never
surface wire messages. Builds without the feature refuse a requested scheduler;
there is no mock fallback. The guarded Apple builds and their evidence are below.

Setup's existing immutable `/code/bin/step.wasm`, provenance and manifest pins
are still required. Use the actual WAMR compute mode
`wamr-2.4.3-fast-cumulative-v1`; unsupported modes and over-budget declared
maximum memory are refused before input consumption. This first route admits `coglet_v0` imports (`emit`, `timer`, `llm`, `log`) and the
restricted WASI preview1 file operations defined in `spec/abi.md`. Preopens are
`/code` (fd 3, read-only), `/data` (fd 4, private read-write) and `/logs` (fd 5,
create/append only) on the coglet-fs overlay. Clock, random, poll, symlink-create
and hard-link imports still fail module admission. LLM
calls can stage requests but this card does not supply a provider/gateway pump.

Each native pump schedules at most one arrival-ordered batch. No guest executes
without current authenticated manifest operation/subscription grants and
exclusive leases. Current grants, actual engine metadata and durable step-derived
seed are recorded, not caller-provided engine metadata. One absolute deadline
covers load, instantiation, initialization, allocation, copy-in and handle. Every
attempt gets a fresh instance; failed attempts discard staged effects and commit
one failure record before consuming the batch. Successful action arrays commit
in invocation order before the transport pump can see them. After execution,
the supervised clock is sampled again for each dispatch authorization; a lease
that expired during guest execution cannot prepare a new envelope.

Usage exposes measured compute/copy bytes and wall duration. Since the shared
engine currently does not report peak linear pages, `memory_pages` conservatively
records the admitted cap, not measured peak/RSS. Native wall duration is
nondeterministic; exact native archive replay/usage comparison remains part of
the serial ABI qualification, not a claim made by the current mock replay tool.
Scheduling requires a connected, healthy authenticated surface; disconnected
background-only execution is not implemented.

`tests/native_operational.rs` executes real Wasm through actual C pointer entry
points and a separate actual cogletd socket process. It checks call-order acts,
trap rollback, persisted usage/build identity, one-time timer consumption, and
byte-identical old-ID reconciliation after clock advance/reopen. A separate
expensive-initializer regression verifies dispatch uses a post-execution clock
when the original grant has expired. This is not full native speech/device I/O
qualification or final operational acceptance.

## Apple platform evidence

The allow-list is exactly `x86_64-unknown-linux-gnu`,
`armv7-unknown-linux-gnueabihf`, `aarch64-apple-darwin`,
`x86_64-apple-darwin`, and `aarch64-apple-ios`; other targets fail at build.
All use the pinned cumulative-metered interpreter. **AOT is rejected everywhere**,
including Linux x86_64: ABI v0 does not qualify AOT compute caps or deadlines.
JIT and AOT remain disabled in CMake; Apple support does not weaken that rule.

| Target | Evidence / boundary |
|---|---|
| arm64 macOS | [Run 45286035005 / apple job 138082473507](https://github.com/softmax-universe/coglet/actions/runs/45286035005/job/138082473507), head `a73a322472c11478857a81c2aa1ab42b8b7c6d3e`: full engine/runtime default and native-engine package tests, AOT refusal, and production echo C-ABI smoke passed, zero ignored. |
| x86_64 macOS | Same green Apple job cross-built engine/runtime with native-engine on the arm64 runner; no Intel execution claimed. |
| arm64 iOS | Same green Apple job built the interpreter runtime static archive and linked the C configure/start/stop probe against the iPhoneOS SDK (arm64, minimum iOS 15.0); no simulator, signed app or device execution claimed. |
| ARMv7 Linux | [Same run / armv7-qemu job 138082473860](https://github.com/softmax-universe/coglet/actions/runs/45286035005/job/138082473860) passed the spike and real engine suites under QEMU; not physical EO1 evidence. |

This is a source-head evidence snapshot; final PR-head checks are recorded on
[PR #62](https://github.com/softmax-universe/coglet/pull/62). Earlier echo smoke
failures in [run 45282117394](https://github.com/softmax-universe/coglet/actions/runs/45282117394)
and [run 45287359414](https://github.com/softmax-universe/coglet/actions/runs/45287359414)
exposed an invalid test assumption: the latter logged a successful synchronous
grants call taking 3.691 seconds, followed by health send `-2` after the frozen
three-second surface inactivity limit. A deterministic 3.1-second post-grants
pause reproduced that refusal locally; removing only the redundant health
exchange preserved the committed-WAL execution proof with the same pause.
The smoke now closes the link after synchronous execution and checks the WAL,
not uninterrupted surface liveness. No runtime timeout or authorization policy
changed. The source of the slow grants call (guest/setup/WAL/scheduling) is not
isolated, and long-call surface liveness remains outside this qualification.

The anchor recovery policy is shared, but continuity scope differs by platform.
Linux samples boot_id and CLOCK_BOOTTIME; macOS samples `kern.bootsessionuuid`
and `mach_continuous_time`. iOS uses an OS-CSPRNG process epoch and the same
checked u128 continuous-time conversion (ADR-0039). These clocks include sleep;
iOS anchored WALs refuse another process before guest admission, not only after
reboot. Entropy failure is sticky and has no identity fallback. CI continuity
proof is not physical sandbox/suspend qualification. Named regressions:

- `ios_process_epoch_is_stable_and_entropy_failure_refuses` checks real OS
  entropy, concurrent one-time initialization and sticky injected failure.
- `same_process_wal_reopen_preserves_epoch_and_elapsed_time` reopens a real WAL
  with the production process epoch; elapsed time advances without rebasing.
- `ios_anchored_wal_refuses_another_process_before_guest_admission` executes a
  separate process with the production epoch helper and retains the WAL bytes.
  Linux uses BOOTTIME for this profile test; Apple uses continuous time.
  This is not an installed iOS execution claim.

- `native_boot_sample_is_stable_and_monotonic` uses the real kernel sampler.
- `same_boot_wal_recovery_preserves_epoch_and_elapsed_time` reopens the real WAL
  with injected same-boot samples.
- `prepared_dispatch_recovery_refuses_new_boot_and_missing_anchor` prepares real
  dispatch bytes, reopens the WAL, rejects a simulated new boot and a persisted
  legacy missing anchor, and verifies same-boot byte-identical reconciliation.

The CI `echo-smoke` feature additionally requires `COGLET_ECHO_SMOKE_WASM` and
fails if missing. It compiles production `coglets/echo` for
`wasm32-unknown-unknown` (no ambient WASI imports), applies the existing R3
publication transform, starts the runtime through `coglet_runtime_start`, runs
its real handle for a due timer under WAMR, stops, and reopens the WAL to verify
one committed step, nonzero metered compute and pinned engine identity. Echo
ignores timer events, so this is **not** a test.echo surface roundtrip; reserved
catalog operations and native WASI refusal are not bypassed. The ordinary
`ffi_real_wasm_commits_call_order_before_dispatch_and_reconciles` test separately
proves real Wasm emissions through the actual C-pointer boundary.

`tools/apple-link-probe.sh` builds the real runtime static archive for
`aarch64-apple-ios` with WAMR, CMake iOS/arm64/iphoneos and deployment target
15.0, then links a C executable against it and the actual iPhoneOS SDK. It
references configure/start/stop, forcing engine and clock symbol resolution;
it does **not** run that executable. iOS sandbox availability of the boot-session
sysctl, suspended-app scheduling, signing, distribution and physical device
qualification remain separate product gates.
