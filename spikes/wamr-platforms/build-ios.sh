#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v xcrun >/dev/null; then
  printf '%s\n' 'Requires Xcode iPhoneOS SDK and Apple clang (xcrun absent); see README.md.' >&2
  exit 2
fi
bash fetch-wamr.sh
sdk=$(xcrun --sdk iphoneos --show-sdk-path)
cmake -S . -B .cache/ios -DCMAKE_SYSTEM_NAME=iOS -DCMAKE_OSX_SYSROOT="$sdk" \
  -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=15.0 \
  -DCMAKE_C_COMPILER="$(xcrun --sdk iphoneos --find clang)" \
  -DCMAKE_ASM_COMPILER="$(xcrun --sdk iphoneos --find clang)" \
  -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY -DCMAKE_BUILD_TYPE=MinSizeRel \
  -DWAMR_ROOT_DIR="$PWD/.cache/wamr"
cmake --build .cache/ios --parallel 2
rustup target add --toolchain 1.91.1 aarch64-apple-ios
WAMR_LIB_DIR="$PWD/.cache/ios" SDKROOT="$sdk" \
  cargo +1.91.1 build --locked --release --target aarch64-apple-ios
