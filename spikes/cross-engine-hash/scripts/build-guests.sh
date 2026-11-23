#!/usr/bin/env bash
# Build the conforming guest and one counterexample guest per rule into $OUT (default: ./out).
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${OUT:-$HERE/out}"
mkdir -p "$OUT"
cd "$HERE/guest"
T=wasm32-wasip1
W=target/$T/release/xhash_guest.wasm

build() { # name features rustflags
  local name="$1" feats="$2" flags="$3"
  RUSTFLAGS="$flags" cargo build --release --target $T --features "$feats" --quiet
  cp "$W" "$OUT/$name.wasm"
  echo "built $name.wasm ($(stat -c %s "$OUT/$name.wasm") bytes)"
}

build conforming ""            ""
build ce-wasi-clock  "wasi-clock"   ""
build ce-wasi-random "wasi-random"  ""
build ce-relaxed-simd "relaxed-simd" "-Ctarget-feature=+simd128,+relaxed-simd"
build ce-threads "threads" "-Ctarget-feature=+atomics,+bulk-memory"
