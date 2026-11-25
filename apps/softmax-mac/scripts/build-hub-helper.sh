#!/usr/bin/env bash
set -euo pipefail
# Build the app-owned transport for both supported CPUs; never inherit a
# single-architecture developer build as the distributable helper.
root="$(cd "$(dirname "$0")/.." && pwd)"
destination="${1:?usage: build-hub-helper.sh /path/to/Contents/Helpers/softmax-hub-link}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/HubTransport/target}"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --locked --release --manifest-path "$root/HubTransport/Cargo.toml" \
    --bin softmax-hub-link --target "$target" -j 2
done
mkdir -p "$(dirname "$destination")"
# A failed build/merge never replaces an existing bundled helper.
staging="$(mktemp "$(dirname "$destination")/.softmax-hub-link.XXXXXX")"
trap 'rm -f "$staging"' EXIT
lipo -create \
  "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/softmax-hub-link" \
  "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/softmax-hub-link" \
  -output "$staging"
# Verify each required slice separately: the installed Apple lipo rejects a
# multi-architecture verification list even for a valid universal artifact.
# Each check remains mandatory before replacing the bundled helper.
lipo "$staging" -verify_arch arm64
lipo "$staging" -verify_arch x86_64
chmod 755 "$staging"
mv -f "$staging" "$destination"
