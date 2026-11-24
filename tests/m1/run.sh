#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
# Native runtime deliberately has no WASI adapter. Same reference Rust source,
# qualified interpreter target, bounded memory and existing R3 preprocessing.
CARGO_TARGET_DIR="$ROOT/target/m1-guests" cargo rustc --locked \
  --manifest-path coglets/echo/Cargo.toml --target wasm32-unknown-unknown \
  --release --lib -- -C link-arg=--max-memory=16777216
cargo run --quiet --locked -p coglet-runtime --features native-engine \
  --example qualify_apple_guest -- \
  "$ROOT/target/m1-guests/wasm32-unknown-unknown/release/coglet_echo.wasm"
# The echo guest expires actions after one real second. Debug host/WAL work
# does not qualify that latency; never relax/rescale its deadline for a pass.
cargo test --release --locked -p coglet-runtime --features m1-tests --test m1 -- --test-threads=1 --nocapture
COGLET_ECHO_SMOKE_WASM="$ROOT/target/m1-guests/wasm32-unknown-unknown/release/coglet_echo.wasm" \
  cargo test --release --locked -p coglet-runtime --features echo-smoke \
  --test native_operational socket_compiled_echo_timer_wake_starts_executes_and_stops -- --nocapture --test-threads=1
