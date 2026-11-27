#!/bin/bash
# Compile the unchanged SDK modules, then the crate-owned diagnostic host.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../../../.." && pwd)
out=$1
mkdir -p "$out"
out=$(cd "$out" && pwd)
swiftc -swift-version 5 -emit-library -emit-module -module-name CogletTypes \
  "$repo"/sdk/surface-swift/Sources/CogletTypes/*.swift \
  "$repo"/sdk/surface-swift/Sources/CogletTypes/Generated/*.swift \
  -emit-module-path "$out/CogletTypes.swiftmodule" -o "$out/libCogletTypes.dylib"
swiftc -swift-version 5 -emit-library -emit-module -module-name CogletSurface \
  -I "$out" -L "$out" -lCogletTypes \
  "$repo"/sdk/surface-swift/Sources/CogletSurface/*.swift \
  -emit-module-path "$out/CogletSurface.swiftmodule" -o "$out/libCogletSurface.dylib"
swiftc -swift-version 5 -I "$out" -L "$out" -lCogletTypes -lCogletSurface \
  -Xlinker -rpath -Xlinker "$out" \
  "$repo/crates/coglet-runtime/tools/ready-output-sdk/main.swift" -o "$out/ready-output-sdk"
