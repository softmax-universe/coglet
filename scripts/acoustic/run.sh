#!/usr/bin/env bash
# M2b real-Mac acoustic proof runner (ADR-0044). Run ON the Mac under test, from any directory.
#
#   scripts/acoustic/run.sh <out-dir> [face-acoustic options...]
#   scripts/acoustic/run.sh runs/foo --barge-runs 10 --capture-runs 10 --stimulus-runs 10 --seed 7
#
# Builds the face-acoustic harness (surfaces/face-mac), wraps it in a signed .app so macOS can
# attribute the microphone permission, sets the output volume (restored on exit), runs the
# frozen conformance speech timelines through the real FaceSurfaceHost + NativeFaceDuplex graph
# and records raw samples to <out-dir>/runs.jsonl + session.json.
#
# First run after a rebuild: macOS asks once to allow "FaceAcoustic" to use the microphone. A person
# at the Mac must answer it (an ad-hoc signature changes with each build; keep FACE_ACOUSTIC_APP to
# reuse one signed bundle across runs and avoid re-prompting).
#
# Environment: ACOUSTIC_VOLUME (0-100, default 90) system output volume during the run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:?usage: run.sh <out-dir> [options]}"; shift || true
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
PKG="$ROOT/surfaces/face-mac"
APP="${FACE_ACOUSTIC_APP:-$HOME/.cache/coglet-face-acoustic/FaceAcoustic.app}"
VOLUME="${ACOUSTIC_VOLUME:-90}"

if [ "${FACE_ACOUSTIC_SKIP_BUILD:-0}" != 1 ]; then
  swift build -c release --package-path "$PKG" --product face-acoustic
  BIN="$(swift build -c release --package-path "$PKG" --show-bin-path)/face-acoustic"
  rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS"
  cp "$BIN" "$APP/Contents/MacOS/face-acoustic"
  cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.softmax.coglet.faceacoustic</string>
<key>CFBundleExecutable</key><string>face-acoustic</string>
<key>CFBundleName</key><string>FaceAcoustic</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
<key>NSMicrophoneUsageDescription</key><string>Coglet Face M2b acoustic proof (speaker-to-microphone latency measurement)</string>
</dict></plist>
PLIST
  codesign --force -s - --identifier com.softmax.coglet.faceacoustic "$APP"
fi

OLD_VOLUME="$(osascript -e 'output volume of (get volume settings)')"
trap 'osascript -e "set volume output volume ${OLD_VOLUME}" >/dev/null 2>&1 || true' EXIT
osascript -e "set volume output volume ${VOLUME}" >/dev/null

rm -f "$OUT/log.txt"
{
  echo "git_sha: $(git -C "$ROOT" rev-parse HEAD)"
  echo "git_dirty: $(git -C "$ROOT" status --porcelain surfaces/face-mac scripts/acoustic spec/fixtures/speech | wc -l | tr -d ' ')"
  echo "host: $(hostname) $(sw_vers -productVersion) $(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
  echo "volume: ${VOLUME} (restored to ${OLD_VOLUME})"
  echo "argv: $*"
} > "$OUT/env.txt"

open -W -n "$APP" --stdout "$OUT/stdout.txt" --stderr "$OUT/stderr.txt" \
  --args --timelines "$ROOT/spec/fixtures/speech/timelines.json" --out "$OUT" "$@" || true
test -f "$OUT/log.txt" || { echo "face-acoustic wrote no log; see $OUT/stderr.txt" >&2; exit 2; }
tail -1 "$OUT/log.txt"
[ "$(tail -1 "$OUT/log.txt")" = complete ] || exit 1
