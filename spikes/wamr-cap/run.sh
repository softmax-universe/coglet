#!/usr/bin/env bash
# Reproducible Linux x86_64 experiment; prerequisites documented in README.md.
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$PWD"
CACHE="$ROOT/.cache"
OUT="${1:-$ROOT/results}"
mkdir -p "$CACHE" "$OUT"
OUT="$(realpath "$OUT")"
PIN=0e47872fb4e57833dbeccf7c312a406fa2dac2bc
VERSION=WAMR-2.4.3
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
    printf 'This pinned compiler runner supports Linux x86_64 only.\n' >&2; exit 1;
}
for cmd in rustup cmake gcc g++ make curl git sha256sum timeout python3; do
    command -v "$cmd" >/dev/null || { printf 'Missing prerequisite: %s\n' "$cmd" >&2; exit 1; }
done
rustup toolchain install 1.91.1 --profile minimal --target wasm32-unknown-unknown
if [[ ! -d "$CACHE/wamr/.git" ]]; then
    git clone --depth 1 --branch "$VERSION" https://github.com/wasm-micro-runtime/wasm-micro-runtime.git "$CACHE/wamr"
fi
[[ "$(git -C "$CACHE/wamr" rev-parse HEAD)" == "$PIN" ]]
[[ -z "$(git -C "$CACHE/wamr" status --porcelain)" ]]
if [[ ! -f "$CACHE/wamrc.tar.gz" ]]; then
    curl --fail --location --retry 3 "https://github.com/wasm-micro-runtime/wasm-micro-runtime/releases/download/$VERSION/wamrc-2.4.3-x86_64-ubuntu-22.04.tar.gz" -o "$CACHE/wamrc.tar.gz"
fi
printf '%s  %s\n' 349c097fe9951da5beed75b015dae8d021a2d111b3f808079747af74b3b736a5 "$CACHE/wamrc.tar.gz" | sha256sum --check
mkdir -p "$CACHE/compiler"
tar -xzf "$CACHE/wamrc.tar.gz" -C "$CACHE/compiler"
cmake -S "$CACHE/wamr/product-mini/platforms/linux" -B "$CACHE/build" \
    -DCMAKE_BUILD_TYPE=Release -DWAMR_BUILD_INSTRUCTION_METERING=1 \
    -DWAMR_BUILD_FAST_INTERP=1 -DWAMR_BUILD_AOT=1 -DWAMR_BUILD_JIT=0 \
    -DWAMR_BUILD_FAST_JIT=0 -DWAMR_BUILD_LIBC_WASI=0 -DWAMR_BUILD_LIBC_BUILTIN=0 \
    -DWAMR_BUILD_THREAD_MGR=1 -DWAMR_BUILD_SHARED_MEMORY=1 \
    -DWAMR_BUILD_SIMD=0 -DWAMR_BUILD_REF_TYPES=0 2>&1 | tee "$OUT/build-config.txt"
cmake --build "$CACHE/build" --target vmlib -j2
export WAMR_LIB_DIR="$CACHE/build"
# Cargo tracks the library directory but not changes inside libiwasm.a.
# Touch build.rs through cargo clean instead of silently reusing stale linkage.
rustup run 1.91.1 cargo clean
rustup run 1.91.1 cargo test --locked
rustup run 1.91.1 cargo build --release --locked
GUEST_FLAGS=(--edition=2021 guest.rs --crate-type=cdylib --target=wasm32-unknown-unknown
    -C opt-level=2 -C panic=abort -C link-arg=-zstack-size=65536
    -C link-arg=--initial-memory=327680 -C link-arg=--max-memory=1048576 -C strip=symbols)
rustup run 1.91.1 rustc "${GUEST_FLAGS[@]}" -o "$OUT/guest.wasm"
rustup run 1.91.1 rustc "${GUEST_FLAGS[@]}" -C target-feature=+atomics,+bulk-memory,+mutable-globals \
    -C link-arg=--shared-memory -o "$OUT/guest-shared.wasm"
for kind in guest guest-shared; do
    "$CACHE/compiler/wamrc" --enable-multi-thread --disable-simd --disable-ref-types \
        --bounds-checks=1 -o "$OUT/$kind.aot" "$OUT/$kind.wasm"
done
{
    date -u --iso-8601=seconds
    uname -a
    lscpu
    rustup run 1.91.1 rustc --version
    gcc --version
    cmake --version
    "$CACHE/compiler/wamrc" --version
    printf 'WAMR_COMMIT=%s\n' "$PIN"
    sha256sum "$OUT"/*.wasm "$OUT"/*.aot
} > "$OUT/provenance.txt"
HOST="$ROOT/target/release/wamr-cap-spike"
# This MUST time out: ordinary AOT has no termination safe points in this release.
set +e
timeout --signal=KILL 1 "$HOST" "$OUT/guest.aot" aot-negative-control "$OUT" > "$OUT/aot-negative-control.txt" 2>&1
status=$?
set -e
printf 'external_kill_exit=%s\n' "$status" >> "$OUT/aot-negative-control.txt"
[[ "$status" == 137 ]]
[[ "$(<"$OUT/aot-negative-control.txt")" == *'negative_control: terminate sent after 10ms'* ]]
for mode in interp aot aot-shared; do
    guest=guest
    [[ "$mode" != aot-shared ]] || guest=guest-shared
    ext=aot
    [[ "$mode" != interp ]] || ext=wasm
    timeout --signal=KILL 30 "$HOST" "$OUT/$guest.$ext" "$mode" "$OUT" 2>&1 | tee "$OUT/$mode.txt"
done
python3 "$ROOT/summarize.py" "$OUT" > "$OUT/summary.json"
printf 'All assertions passed; evidence: %s\n' "$OUT"
