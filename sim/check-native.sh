#!/usr/bin/env bash
# Real daemon + Rust guest + Python SDK acceptance. Run from any directory.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
rustup target add wasm32-unknown-unknown
cargo test --locked --manifest-path coglets/gridbot-pilot/Cargo.toml
cargo rustc --locked --release --lib --target wasm32-unknown-unknown --manifest-path coglets/gridbot-pilot/Cargo.toml -- -C link-arg=--max-memory=16777216
mkdir -p target/sandbox
cp coglets/gridbot-pilot/target/wasm32-unknown-unknown/release/coglet_gridbot_pilot.wasm target/sandbox/pilot.wasm
cargo run --locked -p coglet-runtime --features native-engine --example qualify_apple_guest -- target/sandbox/pilot.wasm
cargo build --release --locked -p coglet-runtime --features native-engine --bin cogletd
CARGO_TARGET_DIR="$PWD/target" cargo build --release --locked --manifest-path sim/native/Cargo.toml
export COGLET_SANDBOX_DAEMON="$PWD/target/release/cogletd"
export COGLET_SANDBOX_PROOF="$PWD/target/release/coglet-sandbox-proof"
export COGLET_SANDBOX_WASM="$PWD/target/sandbox/pilot.wasm"
# CI may not define TMPDIR; retain a short, private subdir under its workspace.
mkdir -p target/sandbox/tmp
export TMPDIR="${TMPDIR:-$PWD/target/sandbox/tmp}"
uv run --project sim --locked pytest sim/tests/test_native.py sim/tests/test_evaluation.py sim/tests/test_gate_artifacts.py sim/tests/test_gate_adversarial.py sim/tests/test_gate_service.py -v -s
