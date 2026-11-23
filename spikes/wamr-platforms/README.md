# WAMR platform spike

Standalone Rust host; intentionally outside the forthcoming product workspace. No
production ABI or legacy implementation is imported. The C shim only configures
WAMR's pool allocator and reads its statistics; Rust loads/instantiates/calls the
guest, sets budgets, checks traps/results and tears down in lifetime order.

## Linux / ARMv7 reproduction

On Ubuntu 24.04 x86-64:

```sh
sudo apt-get update
sudo apt-get install -y cmake gcc-arm-linux-gnueabihf libc6-dev-armhf-cross qemu-user wabt time
rustup toolchain install 1.91.1 --profile minimal
rustup target add --toolchain 1.91.1 armv7-unknown-linux-gnueabihf
bash spikes/wamr-platforms/test-armv7.sh
```

Run from any directory by supplying the script's absolute path. The crate has its
own `[workspace]`, lockfile and dedicated workflow. `fetch-wamr.sh` checks both the
exact WAMR commit and a clean source tree. Build settings are shared with iOS:
classic interpreter + instruction metering; AOT, both JITs, Memory64, SIMD, guest
threads and WASI disabled. Software memory checks are retained. This validates
core Wasm, **not** the future WASI filesystem or full coglet runtime.

The test uses Cortex-A9 QEMU user mode, calls `answer -> 42`, carries an i64 across
its 32-bit halves, rejects growth beyond the declared maximum, writes/reads the
last word of the initial page, traps on an out-of-bounds load, caps an infinite
loop at 10,000 interpreter instructions, and successfully calls again after
clearing the trap and resetting the budget. A 20-second watchdog is a test guard,
not the compute-cap implementation. WAMR's instruction cap is not a host-call
wall-clock deadline and is not asserted equivalent to Wasmtime fuel.

`results/` contains fresh binary sizes, guest assertions, WAMR pool stats and GNU
time output. CI uploads it even on failure. `evidence/` is a labelled local run
from 2026-11-23, not hosted CI evidence; hosted artifacts are authoritative for
that CI run. Pool stats exclude mmap-backed linear memory, Rust allocations,
native stack, shared libraries, and emulator overhead. The static pool reserves
2 MiB BSS whether or not it is all touched. GNU time measures the **host QEMU
process** (and timeout wrapper), not an EO1's native RSS. See ADR-0008 for results.

**EO1 caveat:** Baby Bear `pods/eo1/` describes an i.MX6/ARMv7 device and inherited
Ubuntu 18.04 baseline. The spike uses the Ubuntu 24.04 cross sysroot and needs
GLIBC_2.34. Do not deploy this binary to an older frame. Use a frame-compatible
sysroot/toolchain (or separately validate a static build) before a physical run.
QEMU success is not proof of physical EO1 execution, kernel compatibility or RSS.
No frame or Baby Bear source was changed.

## iOS: stop at the SDK boundary

On this Linux worker `bash build-ios.sh` exits **2** with:

```
Requires Xcode iPhoneOS SDK and Apple clang (xcrun absent); see README.md.
```

There is no Apple iPhoneOS SDK/Apple linker here. Installing the Rust
`aarch64-apple-ios` standard library alone is insufficient to compile Darwin's
platform C/assembly and link its system libraries. An authorized SDK and a
compatible cross linker could in principle cross-build from Linux; that setup is
not supplied or validated here. The supported next environment is a Mac with
full Xcode (not merely Command Line Tools), its license accepted, CMake, Git,
and rustup installed. Exact steps, **not executed in this spike**:

```sh
# User-terminal setup only if Xcode has not already been selected/accepted:
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
xcrun --sdk iphoneos --show-sdk-path
brew install cmake rustup
rustup toolchain install 1.91.1 --profile minimal
bash spikes/wamr-platforms/build-ios.sh
file spikes/wamr-platforms/.cache/ios/libprobe_wamr.a
file spikes/wamr-platforms/target/aarch64-apple-ios/release/wamr-platform-probe
```

If rustup is installed already, skip its Homebrew install; ensure rustup's
`cargo`/`rustc` shims are on PATH. The script builds a **static arm64 iPhoneOS**
archive using Darwin platform code and the same no-AOT/no-JIT CMake settings,
then links the Rust host for `aarch64-apple-ios`. Deployment target is iOS 15.0;
this is a device build, not an arm64 simulator build. No signing is needed for
the archive; device execution requires a signed app/test harness containing the
host/guest, provisioning and a device. The CLI binary is a link probe, not an
installable iOS app. Neither archive, link nor device execution is claimed here.
Do not use upstream's default iOS shared-library CMake project: it enables
additional features and Linux-style link flags we do not need.

## Verdict: PARTIAL

ARMv7 interpreter embedding, i64 values, software bounds and compute caps are
validated under QEMU. iOS stops at the missing SDK with reproducible Mac steps.
Physical EO1 compatibility/performance and iOS device execution remain unproven.
This is enough to keep the portable wasm32 ABI direction, not to remove the
architecture document's physical-platform validation gate.
