#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
xcodegen generate
xcodebuild -project Softmax.xcodeproj -scheme Softmax -configuration Debug \
  -destination 'platform=macOS' -derivedDataPath build build CODE_SIGNING_ALLOWED=NO
bash scripts/sign-dev.sh "$PWD/build/Build/Products/Debug/Softmax.app"
printf 'Open: %s/build/Build/Products/Debug/Softmax.app\n' "$PWD"
