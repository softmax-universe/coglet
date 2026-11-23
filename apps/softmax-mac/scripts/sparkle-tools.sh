#!/bin/bash
set -euo pipefail
# Tools must match the exact framework release used by XcodeGen.
root="$1"
mkdir -p "$root"
curl --fail --location --retry 3 https://github.com/sparkle-project/Sparkle/releases/download/2.10.1/Sparkle-2.10.1.tar.xz -o "$root/sparkle.tar.xz"
printf '%s  %s\n' 1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27 "$root/sparkle.tar.xz" | shasum -a 256 --check
tar -xf "$root/sparkle.tar.xz" -C "$root"
test -x "$root/bin/sign_update"
test -x "$root/bin/generate_appcast"
