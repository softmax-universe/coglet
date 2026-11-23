#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
bash build-armv7.sh
mkdir -p results
bin=target/armv7-unknown-linux-gnueabihf/release/wamr-platform-probe
stat -c 'binary_bytes=%s' "$bin" | tee results/size.txt
arm-linux-gnueabihf-size "$bin" | tee -a results/size.txt
# Host RSS includes QEMU; it is NOT physical ARM process RSS.
/usr/bin/time -v -o results/qemu-host-time.txt \
  timeout 20s qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf "$bin" .cache/guest.wasm | tee results/armv7.txt
grep -q 'pointer_bits=32' results/armv7.txt
grep -q 'after_cap=42' results/armv7.txt
grep -q 'pool_peak_bytes=' results/armv7.txt
