#!/bin/bash
# Universal macOS static runtime; never include test hooks in distributable builds.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${COGLET_XCFRAMEWORK_OUTPUT:-$ROOT/apps/softmax-mac/Runtime/Artifacts}"
FEATURES=native-engine
if [[ "${COGLET_RUNTIME_TEST_HOOKS:-0}" == 1 ]]; then FEATURES+=,ffi-test-hooks; fi
[[ "$(uname -s)" == Darwin ]] || { printf '%s\n' 'Xcode/macOS is required' >&2; exit 1; }
command -v cbindgen >/dev/null || { printf '%s\n' 'Install cargo install cbindgen --version 0.29.2 --locked' >&2; exit 1; }
cd "$ROOT"
rustup target add aarch64-apple-darwin x86_64-apple-darwin
mkdir -p "$OUT/headers" "$OUT/universal"
cbindgen --config crates/coglet-runtime/cbindgen.toml --crate coglet-runtime --output "$OUT/headers/coglet_runtime.h"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --locked --release -p coglet-runtime --no-default-features --features "$FEATURES" --target "$target"
done
lipo -create target/aarch64-apple-darwin/release/libcoglet_runtime.a target/x86_64-apple-darwin/release/libcoglet_runtime.a -output "$OUT/universal/libCogletRuntime.a"
printf 'module CogletRuntime { header "coglet_runtime.h" export * }\n' > "$OUT/headers/module.modulemap"
rm -rf "$OUT/CogletRuntime.xcframework"
xcodebuild -create-xcframework -library "$OUT/universal/libCogletRuntime.a" -headers "$OUT/headers" -output "$OUT/CogletRuntime.xcframework"
# Verify individually: Apple's lipo can parse a second architecture as an input.
for arch in arm64 x86_64; do
  lipo "$OUT/universal/libCogletRuntime.a" -verify_arch "$arch"
done
# Smoke link each architecture against its SDK, not an execution claim for x86_64.
for arch in arm64 x86_64; do
  printf '#include "coglet_runtime.h"\nint main(void){return coglet_runtime_status()==1?0:1;}\n' | xcrun clang -arch "$arch" -x c - -I "$OUT/headers" -L "$OUT/universal" -lCogletRuntime -framework Security -framework CoreFoundation -liconv -o "$OUT/universal/link-$arch"
done
"$OUT/universal/link-$(uname -m)"
