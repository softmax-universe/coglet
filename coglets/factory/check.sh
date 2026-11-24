#!/usr/bin/env bash
# Supplemental Factory guest proof; not DB runtime/recovery conformance.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test --locked -j2 -p coglet-guest-rust --features database --test database
cargo clippy --locked -j2 -p coglet-guest-rust --features database --all-targets -- -D warnings
cargo fmt --manifest-path coglets/factory/Cargo.toml -- --check
CARGO_TARGET_DIR=target cargo test --manifest-path coglets/factory/Cargo.toml --locked -j2
CARGO_TARGET_DIR=target cargo clippy --manifest-path coglets/factory/Cargo.toml --locked --all-targets -j2 -- -D warnings
CARGO_TARGET_DIR=target/factory RUSTFLAGS='-C link-arg=--max-memory=16777216' cargo build --manifest-path coglets/factory/Cargo.toml --locked --target wasm32-wasip1 --release -j2
uv run --with wasmtime --with cbor2 python3 coglets/factory/tests/test_wasm.py -v
