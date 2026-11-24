#!/bin/bash
# Device and Apple-silicon simulator only. Interpreter policy comes from pinned WAMR.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
plan() {
  printf '%s\n' 'aarch64-apple-ios iphoneos arm64' 'aarch64-apple-ios-sim iphonesimulator arm64'
}
if [[ "${1:-}" == --plan && $# == 1 ]]; then plan; exit 0; fi
[[ $# == 0 ]] || { printf '%s\n' 'Usage: build-ios-runtime-xcframework.sh [--plan]' >&2; exit 1; }
[[ "${COGLET_RUNTIME_TEST_HOOKS:-0}" == 0 ]] || { printf '%s\n' 'Refusing distribution with fault-injection hooks' >&2; exit 1; }
[[ "$(uname -s)" == Darwin ]] || { printf '%s\n' 'Xcode/macOS is required' >&2; exit 1; }
command -v cbindgen >/dev/null || { printf '%s\n' 'Install cbindgen 0.29.2 with --locked' >&2; exit 1; }
OUT="${COGLET_IOS_XCFRAMEWORK_OUTPUT:-$ROOT/target/ios-runtime}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
[[ ! -e "$OUT/CogletRuntime.xcframework" ]] || { printf '%s\n' 'Output framework already exists; choose a new output directory' >&2; exit 1; }
cd "$ROOT"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
mkdir -p "$TARGET_DIR"
export CARGO_TARGET_DIR="$(cd "$TARGET_DIR" && pwd)"
mkdir -p "$OUT/headers"
cbindgen --config crates/coglet-runtime/cbindgen.toml --crate coglet-runtime --output "$OUT/headers/coglet_runtime.h"
printf 'module CogletRuntime { header "coglet_runtime.h" export * }\n' > "$OUT/headers/module.modulemap"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
while read -r target sdk arch; do
  IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo build --locked --release -p coglet-runtime --features native-engine --target "$target"
  mkdir -p "$OUT/$target"
  cp "$CARGO_TARGET_DIR/$target/release/libcoglet_runtime.a" "$OUT/$target/libCogletRuntime.a"
  xcrun lipo "$OUT/$target/libCogletRuntime.a" -verify_arch "$arch"
  xcrun nm -g "$OUT/$target/libCogletRuntime.a" > "$OUT/$target/symbols.txt"
  if grep -q ' _coglet_runtime_test_panic$' "$OUT/$target/symbols.txt"; then
    printf '%s\n' 'Refusing distribution with fault-injection hooks' >&2; exit 1
  fi
  clang_target=arm64-apple-ios15.0
  expected_platform=IOS
  if [[ "$sdk" == iphonesimulator ]]; then
    clang_target+=-simulator
    expected_platform=IOSSIMULATOR
  fi
  printf '#include "coglet_runtime.h"\nint main(void){return coglet_runtime_status()==1?0:1;}\n' |
    xcrun --sdk "$sdk" clang -target "$clang_target" -isysroot "$(xcrun --sdk "$sdk" --show-sdk-path)" -x c - -I "$OUT/headers" "$OUT/$target/libCogletRuntime.a" -framework Security -framework CoreFoundation -liconv -o "$OUT/$target/link-probe"
  xcrun vtool -show-build "$OUT/$target/link-probe" > "$OUT/$target/link-platform.txt"
  grep -Eq "platform[[:space:]]+$expected_platform$" "$OUT/$target/link-platform.txt"
done < <(plan)
xcodebuild -create-xcframework \
  -library "$OUT/aarch64-apple-ios/libCogletRuntime.a" -headers "$OUT/headers" \
  -library "$OUT/aarch64-apple-ios-sim/libCogletRuntime.a" -headers "$OUT/headers" \
  -output "$OUT/CogletRuntime.xcframework"
printf '%s\n' "$OUT/CogletRuntime.xcframework"
