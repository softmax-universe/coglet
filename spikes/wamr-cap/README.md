# WAMR compute/deadline/memory spike

Standalone Rust host (minimal WAMR C FFI, no Rust dependencies) and `no_std`
Rust guest. This is an experiment, not the production engine or ABI.

## Run

On **Linux x86_64, glibc >= 2.35**, install Git, curl, a C/C++ toolchain,
GNU make, CMake >= 3.14, GNU coreutils (`timeout`, `sha256sum`, `realpath`),
Python 3, and rustup (on PATH). Then, from the repository root:

```sh
./spikes/wamr-cap/run.sh
```

The script installs/selects Rust **1.91.1** and its `wasm32-unknown-unknown`
target via rustup, fetches pinned WAMR **2.4.3** source and a checksum-verified
upstream Linux `wamrc` release, builds WAMR with two jobs, tests/builds the Rust
host, compiles both guests and runs all assertions. No sudo, WASI SDK or LLVM
installation is needed. First run needs network; cached runs still contact
rustup. Compiler binaries are upstream prebuilt artifacts, not built from source.

`run.sh /absolute/output/path` chooses a different evidence directory (relative
paths are resolved from this spike directory). Default: `results/`. Cache and
Cargo products stay in `.cache/` and `target/`. These are ignored by Git.
The runner intentionally uses an external SIGKILL for the ordinary-AOT negative
control: **one `Killed` line and `external_kill_exit=137` are expected**, not a
successful WAMR interrupt. All positive runs have an external 30-second safety
limit. The entire command exits nonzero on failed assertions/negative control.

The shared-memory guest emits Rust's warning that the `atomics` target feature
is unstable; the toolchain is pinned for this reason. It never creates guest
threads. Shared memory is an explicit experimental ablation, not a proposed
requirement for every coglet.

## What is exercised

- Fast-interpreter instruction caps 1 / 1,000 / 10,000 / 100,000. Three runaway
  budgets repeated 20 times each, checking identical observable progress; clear
  exception and rearm the same exec env, then successfully call finite code.
- Ordinary and shared-memory AOT both **ignore** the interpreter instruction
  cap: finite work still completes with budget 1.
- A watchdog on another native thread requests termination at an absolute
  10-ms call deadline. 50 runaway calls and 50 fresh-instance recovery checks
  for interpreter and shared-memory AOT. The watchdog is cancelled/joined before
  destroying its target; never detach a thread holding an instance pointer.
- An ordinary, non-shared-memory AOT runaway does **not** return despite a
  termination request. External kill after one second is required.
- Five initial memory pages, 16 module-maximum pages; host cap of 8 allows growth
  to 8 then rejects further/overflow growth. A host cap of 4 is **clamped up to
  5**, not rejected. Repeat for interpreter and both AOT variants.
- Warm module / fresh instance: 100 discarded warmups, 10,000 measurements per
  engine. Each timed region includes instantiation, argument allocation, exec-env
  creation, lookup, `handle(1000)` (1,000 volatile table reads), exec-env destruction
  and deinstantiation. Checksum validation, CSV formatting, disk I/O and module
  load/AOT compilation are **outside** that region. Module load is measured once
  separately. No WASI, filesystem snapshot, host imports or product event parsing.
- 100 actual fresh-instance wakes per engine at 10 Hz, with service duration and
  scheduled-arrival-to-completion lag; report how many miss their 100-ms slot.

The guest contains 192 KiB of initialized lookup-table data used by `handle`,
not just a 200-KB custom-section pad. Non-shared Wasm size: 196,974 bytes.
All percentiles use nearest rank. Times are monotonic elapsed wall time, **not
thread CPU time**. Duty estimates describe serial service occupancy at 10 Hz,
not a measured OS CPU percentage or an end-to-end Face benchmark.

## Evidence

`results/` contains per-mode CSVs and stdout, build flags, host/compiler
provenance and guest artifacts with SHA-256 hashes. Each CSV row is
`kind,index,value_ns,detail`:

| kind | value_ns | detail |
|---|---|---|
| `fuel_N` | time until instruction trap | progress at trap |
| `deadline` | watchdog start to call return | terminate request to call return (ns) |
| `fresh` | instantiate + call + drop | returned checksum |
| `paced_10hz` | instantiate + call + drop | scheduled wake to completion (ns) |

To independently aggregate and validate row counts:

```sh
python3 spikes/wamr-cap/summarize.py spikes/wamr-cap/results
```

A captured measured run is committed in `evidence/` (CSV gzip is lossless;
`summarize.py` accepts either). The proposed decision and limitations are in
[`docs/adr/0001-engine-findings-cap.md`](../../docs/adr/0001-engine-findings-cap.md).

## Verdict: PARTIAL

Interpreter metering and interruption are validated for this pinned build.
AOT has no deterministic fuel and requires shared-memory safe points to support
in-process deadline interruption. Memory bounds need a host-side minimum-memory
admission check. Fresh-instance cost is small at 10 Hz on this measured x86_64
host; iOS/ARM, real filesystem snapshots and full application workloads remain
unmeasured. The freeze owner, not this spike, decides the ABI.
