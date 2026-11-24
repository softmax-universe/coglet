#!/usr/bin/env bash
# Explicit native lane: missing native binary/browser is a failure, never a skip.
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_TARGET_DIR="$PWD/target"
cargo fmt --manifest-path tools/visualizer/native/Cargo.toml -- --check
cargo clippy --release --locked --manifest-path tools/visualizer/native/Cargo.toml -- -D warnings
cargo build --release --locked --manifest-path tools/visualizer/native/Cargo.toml
uv sync --project tools/visualizer --locked
uv run --project tools/visualizer --locked playwright install --with-deps chromium
export COGLET_VISUALIZER_NATIVE="$PWD/target/release/coglet-visualizer-native"
export VISUALIZER_PROOF_DIR="$PWD/tools/visualizer/proof/native"
mkdir -p "$VISUALIZER_PROOF_DIR"
uv run --project tools/visualizer --locked pytest tools/visualizer/native/tests -q \
  --junitxml="$VISUALIZER_PROOF_DIR/native.xml"
