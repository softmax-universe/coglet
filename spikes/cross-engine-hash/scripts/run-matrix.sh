#!/usr/bin/env bash
# Strict evidence gate: expected-success commands must succeed, even after printing
# a complete log. Unsupported relaxed SIMD has its own narrow rejection assertion.
# Env: W (xhash-wasmtime), X (xhash-wamr), WC (wamrc), OUT (guests), R (results)
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${OUT:-$HERE/out}"
W="${W:-$HERE/host-wasmtime/target/release/xhash-wasmtime}"
X="${X:-$HERE/host-wamr/build/xhash-wamr}"
WC="${WC:-wamrc}"
R="${R:-$HERE/results}"
mkdir -p "$R"
export XHASH_STATE=1
SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/null}"
say() { echo "$@" | tee -a "$SUMMARY"; }
fail() { say "FAIL: $*"; exit 1; }

# Explicit checks also apply inside command substitution (errexit is not inherited).
prep() {
  local name="$1" mode="$2" src="$OUT/$1.wasm"
  if [ "$mode" = nancanon ]; then
    "$W" nancanon "$src" "$OUT/$name.nc.wasm" 2>"$R/$name.transform.err" || return 1
    src="$OUT/$name.nc.wasm"
  fi
  "$WC" -o "${src%.wasm}.aot" "$src" >"$R/$name.compile.log" 2>&1 || return 1
  echo "$src"
}

run_cfgs() {
  local tag="$1" src="$2"
  "$W" run "$src" --nan-canon off >"$R/$tag.wt" 2>"$R/$tag.wt.err"
  "$W" run "$src" --nan-canon on >"$R/$tag.wtc" 2>"$R/$tag.wtc.err"
  "$X" "$src" >"$R/$tag.wi" 2>"$R/$tag.wi.err"
  "$X" "${src%.wasm}.aot" >"$R/$tag.wa" 2>"$R/$tag.wa.err"
  "$W" run "$src" --nan-canon off >"$R/$tag.wt2" 2>"$R/$tag.wt2.err"
  # Self-comparison validates every produced log, including informational runs.
  for cfg in wt wtc wi wa wt2; do
    cmp_ --expect equal --label "$tag/$cfg: complete valid evidence" \
      first="$R/$tag.$cfg" second="$R/$tag.$cfg"
  done
}

cmp_() { python3 "$HERE/scripts/compare.py" "$@" 2>&1 | tee -a "$SUMMARY"; }

expect_lint() {
  local name="$1" rule="${2:-}" got rc=0
  got="$("$W" lint "$OUT/$name.wasm" 2>&1)" || rc=$?
  say "lint $name: $got"
  if [ -z "$rule" ]; then
    [ "$rc" -eq 0 ] || fail "$name should lint clean"
  else
    [ "$rc" -eq 1 ] || fail "$name expected lint violation exit 1, got $rc"
    echo "$got" | grep -q "^VIOLATION $rule " || fail "$name should violate $rule"
    if echo "$got" | grep '^VIOLATION ' | grep -Ev "^VIOLATION ($rule|R5) "; then
      fail "$name violated an unexpected rule"
    fi
  fi
}

say "# Cross-engine state-hash spike"
say "## A. Conforming guest, rules applied (lint clean + R3 NaN transform)"
expect_lint conforming
src=$(prep conforming nancanon)
run_cfgs conforming "$src"
cmp_ --expect equal --label "conforming: all four configurations" \
  wasmtime="$R/conforming.wt" wasmtime-canon="$R/conforming.wtc" wamr-interp="$R/conforming.wi" wamr-aot="$R/conforming.wa"

say "## B. Counterexamples"
say "### R3 broken: NaN payloads left to the engine"
src=$(prep conforming raw)
run_cfgs ce-nan "$src"
cmp_ --expect differ --label "R3 off: wasmtime vs wamr-interp vs wamr-aot" \
  wasmtime="$R/ce-nan.wt" wamr-interp="$R/ce-nan.wi" wamr-aot="$R/ce-nan.wa"
cmp_ --expect differ --label "R3 off: engine flag alone is not enough" \
  wasmtime-canon="$R/ce-nan.wtc" wamr-interp="$R/ce-nan.wi"

for ce in wasi-clock:R4 wasi-random:R4 threads:R1; do
  name="${ce%%:*}"; rule="${ce##*:}"
  say "### $name -> violates $rule"
  expect_lint "ce-$name" "$rule"
  src=$(prep "ce-$name" nancanon)
  run_cfgs "ce-$name" "$src"
  case "$name" in
    wasi-clock|wasi-random)
      cmp_ --expect differ --label "$name: same engine, same inputs" \
        run1="$R/ce-$name.wt" run2="$R/ce-$name.wt2" ;;
    threads)
      # This does NOT demonstrate a race: the guest uses single-threaded atomics.
      cmp_ --expect equal --label "threads: single-threaded atomics only" \
        wasmtime="$R/ce-$name.wt" wamr-interp="$R/ce-$name.wi" wamr-aot="$R/ce-$name.wa" ;;
  esac
done

say "### relaxed-simd -> violates R2 (explicit load rejection, not hash divergence)"
expect_lint ce-relaxed-simd R2
src="$OUT/ce-relaxed-simd.nc.wasm"
"$W" nancanon "$OUT/ce-relaxed-simd.wasm" "$src" 2>"$R/ce-relaxed-simd.transform.err"
"$W" run "$src" --nan-canon off >"$R/ce-relaxed-simd.wt" 2>"$R/ce-relaxed-simd.wt.err"
cmp_ --expect equal --label "relaxed-simd: Wasmtime completed" \
  first="$R/ce-relaxed-simd.wt" second="$R/ce-relaxed-simd.wt"
rc=0
"$X" "$src" >"$R/ce-relaxed-simd.wi" 2>"$R/ce-relaxed-simd.wi.err" || rc=$?
[ "$rc" -eq 3 ] || fail "relaxed-simd expected load rejection exit 3, got $rc"
[ ! -s "$R/ce-relaxed-simd.wi" ] || fail "relaxed-simd unexpectedly executed steps"
grep -Fxq 'load failed: WASM module load failed: invalid opcode 0xfd 105.' "$R/ce-relaxed-simd.wi.err" \
  || fail "relaxed-simd failed for an unexpected reason"
say "PASS: R2 WAMR rejected unsupported relaxed-SIMD opcode 0xfd 105"
say "RESULT: all expectations met"
