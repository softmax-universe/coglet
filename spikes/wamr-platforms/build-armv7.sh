#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
bash fetch-wamr.sh
cmake -S . -B .cache/armv7 -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR=armv7 \
  -DCMAKE_C_COMPILER=arm-linux-gnueabihf-gcc -DCMAKE_ASM_COMPILER=arm-linux-gnueabihf-gcc \
  -DCMAKE_BUILD_TYPE=MinSizeRel -DWAMR_ROOT_DIR="$PWD/.cache/wamr"
cmake --build .cache/armv7 --parallel 2
wat2wasm guest.wat -o .cache/guest.wasm
WAMR_LIB_DIR="$PWD/.cache/armv7" CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc \
  cargo +1.91.1 build --locked --release --target armv7-unknown-linux-gnueabihf
