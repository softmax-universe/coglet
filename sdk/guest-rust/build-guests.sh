#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export CARGO_TARGET_DIR="$ROOT/target/guests"
for guest in echo counter; do
  cargo rustc --manifest-path "$ROOT/coglets/$guest/Cargo.toml" \
    --target wasm32-wasip1 --release --lib --locked -- \
    -C link-arg=--max-memory=16777216
done
# Rust/serde retain float operators even when envelopes reject floats. R3 is
# publication preprocessing, not an engine admission exemption.
CARGO_TARGET_DIR="$ROOT/target" cargo run --quiet --locked \
  --manifest-path "$ROOT/Cargo.toml" -p coglet-guest-rust --example qualify_guest -- \
  "$ROOT/target/guests/wasm32-wasip1/release/coglet_echo.wasm" \
  "$ROOT/target/guests/wasm32-wasip1/release/coglet_counter.wasm"
