#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
sha=8c18e3f68b16c4bcaf05996b2636f6ed2b4cf629
if [[ ! -d .cache/wamr/.git ]]; then
  mkdir -p .cache
  git clone --depth 1 --branch WAMR-2.4.4 https://github.com/bytecodealliance/wasm-micro-runtime.git .cache/wamr
fi
[[ "$(git -C .cache/wamr rev-parse HEAD)" == "$sha" ]]
[[ -z "$(git -C .cache/wamr status --porcelain)" ]]
