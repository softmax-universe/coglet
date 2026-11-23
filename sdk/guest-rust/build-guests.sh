#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export CARGO_TARGET_DIR="$ROOT/target/guests"
for guest in echo counter; do
  cargo rustc --manifest-path "$ROOT/coglets/$guest/Cargo.toml" \
    --target wasm32-wasip1 --release --lib --locked -- \
    -C link-arg=--max-memory=16777216
 done
