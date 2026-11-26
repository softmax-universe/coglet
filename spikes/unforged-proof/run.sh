#!/usr/bin/env bash
# Coglet x unforged proof spike. Builds the real coglets/counter guest, the
# harness (coglet crates + unforged crates) and the pinned coglet-wamr-step helper
# in a disposable directory, then runs Acts 0-4. Exit 0 only if every check and
# `git fsck --full` pass.
#
# Usage: bash spikes/unforged-proof/run.sh [--json <path>] [--html <path>] [--verbose] [--keep]
#
# Environment:
#   UNFORGED_DIR   unforged checkout to use (must contain crates/unforged-gitobj).
#                  Default: ../unforged if it is that commit or has the crate, else a
#                  temporary worktree/clone of UNFORGED_REV.
#   UNFORGED_REV   unforged commit to pin (default below).
#   SPIKE_TARGET   disposable build root (default /tmp/coglet-spike-target).
set -euo pipefail

UNFORGED_REV="${UNFORGED_REV:-5c1b48aa52a697885812c4ae4d5d73ae6ea94ad1}"
UNFORGED_URL="https://github.com/softmax-universe/unforged"
SPIKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COGLET_DIR="$(cd "$SPIKE_DIR/../.." && pwd)"
SPIKE_TARGET="${SPIKE_TARGET:-/tmp/coglet-spike-target}"
KEEP=0
HARNESS_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --json|--html)
      if [ $# -lt 2 ]; then
        echo "run.sh: $1 needs a path" >&2
        exit 2
      fi
      HARNESS_ARGS+=("$1" "$(cd "$(dirname "$2")" && pwd)/$(basename "$2")")
      shift 2
      ;;
    --verbose) HARNESS_ARGS+=("--verbose"); shift ;;
    --keep) KEEP=1; shift ;;
    *) echo "run.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

log() { printf '==> %s\n' "$*" >&2; }

# ---------------------------------------------------------------------------
# Resolve the unforged checkout.
# ---------------------------------------------------------------------------
TEMP_UNFORGED=""
TEMP_UNFORGED_PARENT=""
cleanup() {
  if [ -n "$TEMP_UNFORGED" ] && [ -n "$TEMP_UNFORGED_PARENT" ]; then
    git -C "$TEMP_UNFORGED_PARENT" worktree remove --force "$TEMP_UNFORGED" >/dev/null 2>&1 || true
  fi
  if [ -n "$TEMP_UNFORGED" ] && [ -d "$TEMP_UNFORGED" ]; then
    rm -rf "$TEMP_UNFORGED"
  fi
}
trap cleanup EXIT

if [ -z "${UNFORGED_DIR:-}" ]; then
  # Candidates: a sibling of this checkout, then a sibling of the primary checkout
  # when this is a linked worktree. A candidate at the pinned commit is used as is;
  # one that merely contains the commit gets a detached temporary worktree.
  candidates=("$COGLET_DIR/../unforged")
  common="$(git -C "$COGLET_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
  if [ -n "$common" ]; then
    candidates+=("$(dirname "$common")/../unforged")
  fi
  for sibling in "${candidates[@]}"; do
    if [ -n "${UNFORGED_DIR:-}" ]; then
      break
    fi
    if [ ! -e "$sibling/.git" ]; then
      continue
    fi
    head="$(git -C "$sibling" rev-parse HEAD 2>/dev/null || true)"
    if [ "$head" = "$UNFORGED_REV" ]; then
      UNFORGED_DIR="$(cd "$sibling" && pwd)"
    elif git -C "$sibling" cat-file -e "$UNFORGED_REV^{commit}" 2>/dev/null; then
      TEMP_UNFORGED="$(mktemp -d "${TMPDIR:-/tmp}/unforged-proof.XXXXXX")"
      rmdir "$TEMP_UNFORGED"
      TEMP_UNFORGED_PARENT="$(cd "$sibling" && pwd)"
      log "creating temporary unforged worktree at $UNFORGED_REV from $TEMP_UNFORGED_PARENT"
      git -C "$sibling" worktree add --detach "$TEMP_UNFORGED" "$UNFORGED_REV" >/dev/null 2>&1
      UNFORGED_DIR="$TEMP_UNFORGED"
    fi
  done
fi
if [ -z "${UNFORGED_DIR:-}" ]; then
  TEMP_UNFORGED="$(mktemp -d "${TMPDIR:-/tmp}/unforged-proof.XXXXXX")"
  log "cloning $UNFORGED_URL at $UNFORGED_REV (private repository: needs access)"
  git clone --quiet "$UNFORGED_URL" "$TEMP_UNFORGED"
  git -C "$TEMP_UNFORGED" checkout --quiet --detach "$UNFORGED_REV"
  UNFORGED_DIR="$TEMP_UNFORGED"
fi
if [ ! -d "$UNFORGED_DIR/crates/unforged-gitobj" ]; then
  echo "run.sh: $UNFORGED_DIR has no crates/unforged-gitobj" >&2
  exit 2
fi
UNFORGED_HEAD="$(git -C "$UNFORGED_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
COGLET_HEAD="$(git -C "$COGLET_DIR" rev-parse HEAD)"
log "coglet   $COGLET_DIR @ $COGLET_HEAD"
log "unforged $UNFORGED_DIR @ $UNFORGED_HEAD"

# ---------------------------------------------------------------------------
# Toolchain: the coglet pin, plus wasm32-wasip1 for the guest.
# ---------------------------------------------------------------------------
TOOLCHAIN="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$COGLET_DIR/rust-toolchain.toml")"
export RUSTUP_TOOLCHAIN="$TOOLCHAIN"
if command -v rustup >/dev/null 2>&1; then
  installed="$(rustup target list --installed --toolchain "$TOOLCHAIN" 2>/dev/null || true)"
  case "$installed" in
    *wasm32-wasip1*) ;;
    *)
      log "installing wasm32-wasip1 for $TOOLCHAIN"
      rustup target add --toolchain "$TOOLCHAIN" wasm32-wasip1
      ;;
  esac
fi
for tool in git cmake cc ssh-keygen; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "run.sh: $tool is required" >&2
    exit 2
  fi
done
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0

# ---------------------------------------------------------------------------
# 1. Real counter guest (same recipe as sdk/guest-rust/build-guests.sh).
# ---------------------------------------------------------------------------
mkdir -p "$SPIKE_TARGET"
log "building coglets/counter for wasm32-wasip1"
CARGO_TARGET_DIR="$SPIKE_TARGET/guests" cargo rustc --quiet \
  --manifest-path "$COGLET_DIR/coglets/counter/Cargo.toml" \
  --target wasm32-wasip1 --release --lib --locked -- \
  -C link-arg=--max-memory=16777216
WASM="$SPIKE_TARGET/guests/wasm32-wasip1/release/coglet_counter.wasm"

# ---------------------------------------------------------------------------
# 2. Materialize the harness manifest outside the coglet tree and build it.
# ---------------------------------------------------------------------------
BUILD="$SPIKE_TARGET/build"
mkdir -p "$BUILD"
esc() { printf '%s' "$1" | sed -e 's/[\/&|]/\\&/g'; }
sed -e "s|@SPIKE_DIR@|$(esc "$SPIKE_DIR")|g" \
    -e "s|@COGLET_DIR@|$(esc "$COGLET_DIR")|g" \
    -e "s|@UNFORGED_DIR@|$(esc "$UNFORGED_DIR")|g" \
    "$SPIKE_DIR/Cargo.toml.in" > "$BUILD/Cargo.toml"
LOCK_FLAG=""
if [ -f "$SPIKE_DIR/Cargo.lock.in" ]; then
  cp "$SPIKE_DIR/Cargo.lock.in" "$BUILD/Cargo.lock"
  LOCK_FLAG="--locked"
fi
log "building harness + coglet-wamr-step (coglet crates x unforged crates)"
# The spike's own crate denies warnings in source (#![deny(warnings)]); RUSTFLAGS
# would also apply -D warnings to the uncapped path dependencies.
(cd "$BUILD" && CARGO_TARGET_DIR="$SPIKE_TARGET/target" cargo build --quiet $LOCK_FLAG)
BIN="$SPIKE_TARGET/target/debug"

# ---------------------------------------------------------------------------
# 3. Run the proof.
# ---------------------------------------------------------------------------
status=0
"$BIN/coglet-unforged-proof" \
  --work "$SPIKE_TARGET/work" \
  --wasm "$WASM" \
  --manifest "$COGLET_DIR/coglets/counter/manifest.json" \
  --helper "$BIN/coglet-wamr-step" \
  --coglet-head "$COGLET_HEAD" \
  --coglet-rev "$COGLET_HEAD" \
  --unforged-rev "$UNFORGED_HEAD" \
  --git-version "$(git --version)" \
  ${HARNESS_ARGS[@]+"${HARNESS_ARGS[@]}"} || status=$?

if [ "$KEEP" -eq 0 ]; then
  rm -rf "$SPIKE_TARGET/work"
fi
exit "$status"
