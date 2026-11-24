#!/usr/bin/env bash
# SDK link evidence only: no signing, simulator or device execution.
set -euo pipefail
export IPHONEOS_DEPLOYMENT_TARGET=15.0
cargo build --locked -p coglet-runtime --lib --no-default-features --features native-engine --target aarch64-apple-ios
sdk="$(xcrun --sdk iphoneos --show-sdk-path)"
xcrun --sdk iphoneos clang -arch arm64 -isysroot "$sdk" \
  -miphoneos-version-min=15.0 crates/coglet-runtime/tools/apple-link-probe.c \
  target/aarch64-apple-ios/debug/libcoglet_runtime.a \
  -framework Security -framework CoreFoundation -liconv -lresolv -lpthread -lm -ldl \
  -o target/aarch64-apple-ios/link-probe
xcrun lipo -info target/aarch64-apple-ios/link-probe
xcrun vtool -show-build target/aarch64-apple-ios/link-probe
