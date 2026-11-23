# ADR-0001: WAMR compute caps, interruption, fresh wakes and memory

- Status: **proposed**
- Date: 2026-11-23
- Scope: findings for the v0 freeze, **not** acceptance of an ABI change
- Experiment: [`spikes/wamr-cap/`](../../spikes/wamr-cap/README.md)

## Context and version

The revised runtime model asks whether WAMR can cap each step and interrupt a
runaway while retaining fresh-instance-per-wake operation. This spike is a clean,
standalone Rust host using minimal C FFI and a Rust core-Wasm guest. It does not
implement the product engine, WASI filesystem or proposed coglet ABI.

Measured WAMR **2.4.3**, tag `WAMR-2.4.3`, exact source commit
`0e47872fb4e57833dbeccf7c312a406fa2dac2bc`. Runtime built from source; AOT compiler
is the upstream `wamrc-2.4.3-x86_64-ubuntu-22.04.tar.gz`, SHA-256
`349c097fe9951da5beed75b015dae8d021a2d111b3f808079747af74b3b736a5`.
Rust **1.91.1** (`ed61e7d7e`), GCC 13.3.0, CMake 4.4.5. Host: Ubuntu Linux
x86_64 VM, Intel Xeon Platinum 8488C, 16 visible logical CPUs. No core pinning,
frequency control or exclusive-host claim. AOT targets the compiler-detected
`sapphirerapids` host CPU; artifacts are not portable to arbitrary CPUs.

The recorded run started at `2026-11-23T00:45:15+00:00` (host UTC). WAMR's own
warning-log clock differs; it is not used for latency. See committed
[provenance](../../spikes/wamr-cap/evidence/provenance.txt),
[build configuration](../../spikes/wamr-cap/evidence/build-config.txt),
[summary](../../spikes/wamr-cap/evidence/summary.json), and losslessly compressed
raw [interpreter](../../spikes/wamr-cap/evidence/interp.csv.gz),
[AOT](../../spikes/wamr-cap/evidence/aot.csv.gz), and
[shared-memory AOT](../../spikes/wamr-cap/evidence/aot-shared.csv.gz) samples.

## Findings

| Requirement | Verdict | Measured result |
|---|---|---|
| Deterministic per-call compute limit | **works-with-caveat** in interpreter; **doesn't** in AOT | Interpreter caps 1,000 / 10,000 / 100,000 repeatedly stop at progress 250 / 2,500 / 25,000. Each budget: 20 identical trials. Both AOT variants complete finite work even with limit 1. |
| Deadline interrupt + host recovery | **works-with-caveat** | Interpreter and shared-memory AOT: 50/50 terminate and 50/50 fresh recovery each. Ordinary non-shared AOT ignores terminate; external process kill required. |
| Fresh instance and approximately 10 events/s | **works-with-caveat** | 10,000 fresh-instance samples per engine below; 100 actual 10-Hz wakes per engine, zero missed 100-ms slots. This is small-host-work microbenchmark evidence, not a Face end-to-end SLA. |
| Max linear memory pages | **works-with-caveat** | Cap 8 enforced for growth from initial 5 pages; next-page and overflow growth return -1. Cap 4 is raised to initial 5 pages rather than rejecting instantiation. |

### 1. Interpreter compute budget, not portable engine fuel

Build with `WAMR_BUILD_INSTRUCTION_METERING=1` (off by default), then call
`wasm_runtime_set_instruction_count_limit(exec_env, positive_i32)` **before each
call**. A finite `handle(1000)` traps with limit 1. The runaway increments a
volatile guest counter; the interpreter traps with an instruction-related
exception. Clear the exception, set a new budget on that same execution
environment, and finite code runs successfully in all 60 trials.

| Interpreter budget | Trials | Identical counter at trap | Trap elapsed p50 (ns) | p99 (ns) |
|---|---:|---:|---:|---:|
| 1,000 | 20 | 250 | 3,531 | 5,708 |
| 10,000 | 20 | 2,500 | 16,127 | 17,919 |
| 100,000 | 20 | 25,000 | 143,604 | 164,133 |

This is the **fast interpreter's internal instruction accounting**, not canonical
Wasm-opcode fuel stable across interpreter implementations, optimization settings
or engines. It does not account for arbitrary time spent in a host import.
The build/version must be part of any reproducibility claim. Counter equality
in this guest is evidence of deterministic cutoff for this build, not a proof
of cross-platform state-hash equivalence for every guest.

Both ordinary AOT and shared-memory AOT ignore this setter: with limit 1,
`handle(1000)` returns the expected checksum instead of trapping. Source confirms
metering sites in the two interpreters, not the AOT compiler.

**ABI consequence:** do not promise a single deterministic fuel unit across
WAMR interpreter/AOT/Wasmtime. If deterministic cutoff is mandatory, the pinned
interpreter is the currently proven route; AOT needs separately qualified
instrumentation or must reject that budget mode. A wall-clock budget is an
available AOT alternative only with the interruption caveat below, and is
**not deterministic replay fuel**. Freeze must distinguish engine-specific
compute metering from a wall-clock execution deadline.

### 2. Deadline interruption requires AOT safe points

The Rust watchdog targets an absolute deadline 10 ms after the timed call setup
begins. A native thread calls `wasm_runtime_terminate(instance)`; the calling
thread returns with a termination exception. The watchdog is cancelled and
joined **before** deinstantiation, preventing late-pointer use. A fresh instance
then returns a valid finite result (50/50 for each supported variant).

| Variant | 10-ms deadline to return p50 (ns) | p99 (ns) | Terminate-request to return p50 (ns) | p99 (ns) |
|---|---:|---:|---:|---:|
| Interpreter | 10,070,879 | 10,374,962 | 3,258 | 4,979 |
| Shared-memory AOT | 10,071,691 | 10,318,051 | 3,486 | 7,534 |

`n=50`, nearest-rank p99 is the maximum in these samples. The first interval
includes watchdog creation/scheduling; the second starts immediately before the
terminate API invocation and ends on call return, **not** after thread join or
drop. These are scheduler-dependent observations, not a hard real-time bound.

**Important negative control:** an ordinary Rust guest with non-shared linear
memory still loops after the termination request, even when compiled with
`--enable-multi-thread`. The reproducible runner logs the request at 10 ms and
requires external SIGKILL at 1 s (exit 137). An earlier exploratory run also had
to be killed by a 30-second external timeout. No in-process recovery is claimed
for that case. See [retained negative-control log](../../spikes/wamr-cap/evidence/aot-negative-control.txt).

At the pinned source, `check_suspend_flags` in
[`aot_emit_control.c`](https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/0e47872fb4e57833dbeccf7c312a406fa2dac2bc/core/iwasm/compilation/aot_emit_control.c#L878-L917)
returns without generating checks unless the guest memory is **shared**.
The measured successful AOT ablation uses all of:

- guest built with `+atomics,+bulk-memory,+mutable-globals` and `--shared-memory`;
- `wamrc --enable-multi-thread` (plus explicit software bounds checks);
- runtime `WAMR_BUILD_THREAD_MGR=1` and `WAMR_BUILD_SHARED_MEMORY=1`.

There are no guest threads in this experiment. Rust warns that its `atomics`
target feature is unstable; pinning the compiler is intentional.

**ABI consequence:** ordinary core-Wasm AOT is **not** currently safe for
untrusted runaway code in the runtime process. Do not silently claim
`wasm_runtime_terminate` is sufficient. Options for freeze: interpreter-only
initial execution; a qualified shared-memory AOT build contract; separately
qualified compiler instrumentation; or process isolation with an external kill.
This ADR proposes no mandatory shared-memory ABI change.

The probe caps **exported calls**, not loader/instantiation start-function code,
and tests no blocking imports. Production admission must forbid/otherwise bound
start functions, and host calls must have their own cancellation/deadline rules.
A timer unable to run because of host scheduler starvation is not a hard bound.

### 3. Fresh-instance latency and event rate

Rust guest: non-shared Wasm **196,974 bytes**, ordinary AOT **198,196 bytes**,
shared-memory AOT **198,824 bytes**. The guest contains 192 KiB of initialized
lookup-table data actually read by `handle`, not only a size-padding custom
section. It starts with 5 memory pages. Work per wake: 1,000 volatile table reads
and a checked integer checksum, no imports/WASI.

Module load is outside the per-wake region and occurs once: interpreter 37,575 ns,
ordinary AOT 64,740 ns, shared AOT 51,877 ns. AOT compilation is also excluded.
Timed region: instantiation (including args), exec-env creation, export lookup,
call, exec-env destruction and deinstantiation. Result checking/CSV work is
outside. Warm module, 100 discarded warmups, **10,000 timed fresh instances per
variant**, monotonic elapsed time, nearest-rank percentiles:

| Variant | Fresh wake p50 (ns) | p99 (ns) | Max (ns) | Mean (ns) | Mean service occupancy at 10 Hz |
|---|---:|---:|---:|---:|---:|
| Interpreter | 74,946 | 122,434 | 284,408 | 78,085.9 | 0.07809% |
| Ordinary AOT (no interrupt guarantee) | 67,347 | 95,861 | 167,818 | 68,554.9 | 0.06855% |
| Shared-memory AOT | 70,462 | 116,197 | 222,816 | 73,575.7 | 0.07358% |

Occupancy = mean elapsed seconds × events/second; it is **not** OS CPU usage.
Allocation, memory initialization and destruction dominate this small workload;
this table does not establish which engine wins for substantial guest compute.

A separate, actually paced test delivers **100 wakes at 10 Hz** to each engine:

| Variant | Paced service p50 / p99 (ns) | Scheduled wake to completion p99 (ns) | Missed 100-ms slots |
|---|---:|---:|---:|
| Interpreter | 189,877 / 238,683 | 514,165 | 0/100 |
| Ordinary AOT | 179,826 / 211,436 | 513,429 | 0/100 |
| Shared-memory AOT | 184,693 / 219,411 | 483,342 | 0/100 |

Paced wake costs are higher than tight-loop costs (cache/scheduler effects);
neither is hidden by mixing sample populations. 100 arrivals span about 9.9 s
because the first arrival is immediate. **Approximately 10 events/s is affordable
for this synthetic workload on this host.** No promise for iOS, EO1/ARM, many
concurrent coglets, filesystem tree snapshots, event decoding, journaling, LLMs,
audio processing or a real Face workload. Those remain product acceptance tests.

### 4. Memory admission and growth

`wasm_runtime_instantiation_args_set_max_memory_pages(args, 8)` applies through
`wasm_runtime_instantiate_ex2`. Guest minimum 5, declared maximum 16: grow to 8
succeeds, growth to 9 returns Wasm -1 without changing the size. Growth with
`u32::MAX` also fails. All three variants pass.

A host cap **below the module minimum is not an instantiation failure**: cap 4
still allocates 5 pages and disallows further growth. The source helper
[`wasm_runtime_get_max_mem`](https://github.com/wasm-micro-runtime/wasm-micro-runtime/blob/0e47872fb4e57833dbeccf7c312a406fa2dac2bc/core/iwasm/common/wasm_runtime_common.c#L1599-L1620)
clamps upward to the minimum; cap 0 means use the module maximum, not zero memory.

**ABI consequence:** reject modules whose initial memory exceeds the granted
page budget **before instantiation**; validate memory layout/count and disallow
uncontrolled imports. A max-linear-pages setting alone is not a total host-memory
cap: module code, exec stack, native metadata, VM reservations and filesystem
state require separate accounting. This spike is single-memory only.

## Reproduction and proposed next decision

From the repository root, on Linux x86_64 with README prerequisites:

```sh
./spikes/wamr-cap/run.sh
python3 spikes/wamr-cap/summarize.py spikes/wamr-cap/results
# Independently recompute the retained historical samples:
python3 spikes/wamr-cap/summarize.py spikes/wamr-cap/evidence
```

The standalone crate intentionally stays outside the root workspace; no shared
CI/workspace edits are required. The runner fails on assertion/count mismatches
and preserves raw samples and provenance. It reports performance rather than
asserting an arbitrary p99 SLA. The recorded run passed its unit test, 60 fuel
trials, 100 deadline/recovery pairs, 30,000 timed fresh wakes, 300 paced wakes,
memory positive/negative controls and ordinary-AOT external-kill control.

**Recommendation (proposed):** qualify interpreter first; expose engine capability
flags for deterministic metering and in-process interruption, reject unsupported
budget modes, and require host-side minimum-memory admission. Keep AOT optional
until freeze explicitly chooses and tests its safe-point or isolation contract.
Do not use a wall-clock cutoff to assert deterministic replay/state hashes; journal
the timeout outcome and avoid committing partial effects from failed steps.
The v0 freeze card owns that decision; this ADR remains **proposed**.
