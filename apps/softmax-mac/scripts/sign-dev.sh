#!/usr/bin/env bash
set -euo pipefail
# Development only. Release signing/notarization is a separate pipeline.
app_dir="${1:?usage: sign-dev.sh /path/to/Softmax.app}"
[[ -d "$app_dir/Contents" ]] || { printf 'Not an app bundle: %s\n' "$app_dir" >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
pin="${SOFTMAX_DEV_IDENTITY_FILE:-$root/.dev-signing-identity}"
identities="$(security find-identity -v -p codesigning)"
identity="${SOFTMAX_DEV_SIGNING_IDENTITY:-}"
if [[ -z "$identity" && -f "$pin" ]]; then
  IFS= read -r identity < "$pin"
  [[ -n "$identity" && "$identity" != '-' ]] || { printf 'Invalid pinned identity: %s\n' "$pin" >&2; exit 1; }
fi
if [[ -z "$identity" ]]; then
  identity="$(printf '%s\n' "$identities" | python3 -c '
import re,sys
found=list(dict.fromkeys(re.findall(r"^\s*\d+\)\s+([0-9A-Fa-f]{40})\s",sys.stdin.read(),re.M)))
print(found[0] if len(found)==1 else "-")
')"
fi
if [[ "$identity" != '-' ]]; then
  # Require a certificate fingerprint, not a potentially ambiguous common name.
  [[ "$identity" =~ ^[0-9A-Fa-f]{40}$ ]] && [[ "$identities" == *"$identity"* ]] || {
    printf 'Selected/pinned signing identity is unavailable. Restore its keychain private key or explicitly select SOFTMAX_DEV_SIGNING_IDENTITY (40-character SHA-1). No silent identity rotation.\n' >&2
    exit 1
  }
  printf '%s\n' "$identity" > "$pin"
else
  printf 'WARNING: ad-hoc fallback; microphone grants may NOT survive rebuilds. Set SOFTMAX_DEV_SIGNING_IDENTITY to a stable keychain certificate fingerprint.\n' >&2
fi
# Inside out: never --deep-sign the app and hope embedded code was handled.
shopt -s nullglob
for code in "$app_dir"/Contents/Frameworks/*.framework "$app_dir"/Contents/Frameworks/*.dylib; do
  codesign --force --sign "$identity" --timestamp=none "$code"
done
codesign --force --sign "$identity" --timestamp=none "$app_dir"
codesign --verify --deep --strict --verbose=2 "$app_dir"
codesign -d -r- "$app_dir"
printf 'Dev identity: %s\n' "$identity"
