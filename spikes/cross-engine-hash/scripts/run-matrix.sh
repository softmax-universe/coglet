#!/usr/bin/env bash
# Runs the whole spike matrix and fails if any expectation is violated.
#   PASS-CONFORMING: guest built per ABI rules R1-R5 -> identical per-step hashes on
#                    wasmtime, wasmtime+canonicalization, wamr interpreter, wamr AOT
#   COUNTEREXAMPLES: each one breaks exactly one rule; the lint names the rule and the run shows the effect
# Env: W (xhash-wasmtime), X (xhash-wamr), WC (wamrc), OUT (guests, default ./out), R (results dir)
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${OUT:-$HERE/out}"
W="${W:-$HERE/host-wasmtime/target/release/xhash-wasmtime}"
X="${X:-$HERE/host-wamr/build/xhash-wamr}"
WC="${WC:-wamrc}"
R="${R:-$HERE/results}"
mkdir -p "$R"
export XHASH_STATE=1
FAILS=0
SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/null}"

say() { echo "$@" | tee -a "$SUMMARY"; }
fail() { say "FAIL: $*"; FAILS=$((FAILS + 1)); }

# prep NAME RAW -> $OUT/NAME.wasm and NAME.aot; RAW=nancanon applies the R3 transform first
prep() {
  local name="$1" mode="$2" src="$OUT/$1.wasm"
  if [ "$mode" = nancanon ]; then
    "$W" nancanon "$src" "$OUT/$name.nc.wasm" 2>/dev/null && src="$OUT/$name.nc.wasm"
  fi
  "$WC" -o "${src%.wasm}.aot" "$src" >/dev/null 2>&1 || rm -f "${src%.wasm}.aot"
  echo "$src"
}

# run_cfgs TAG SRC.wasm -> $R/TAG.{wt,wtc,wi,wa}  (missing/failed runs leave a file with the error)
run_cfgs() {
  local tag="$1" src="$2"
  "$W" run "$src" --nan-canon off >"$R/$tag.wt" 2>"$R/$tag.wt.err" || true
  "$W" run "$src" --nan-canon on  >"$R/$tag.wtc" 2>"$R/$tag.wtc.err" || true
  "$X" "$src" >"$R/$tag.wi" 2>"$R/$tag.wi.err" || true
  "$X" "${src%.wasm}.aot" >"$R/$tag.wa" 2>"$R/$tag.wa.err" || true
  "$W" run "$src" --nan-canon off >"$R/$tag.wt2" 2>/dev/null || true # second run, same engine
  for f in "$R/$tag".*.err; do [ -s "$f" ] && say "  note: $(basename "$f"): $(head -c 160 "$f" | tr '\n' ' ')"; done
}

cmp_() { python3 "$HERE/scripts/compare.py" "$@" 2>&1 | tee -a "$SUMMARY"; return "${PIPESTATUS[0]}"; }

expect_lint() { # NAME RULE... : lint must report exactly these rules (R5 is allowed as a secondary hit)
  local name="$1"; shift
  local got; got="$("$W" lint "$OUT/$name.wasm" 2>&1)"; local rc=$?
  say "lint $name: $(echo "$got" | tr '\n' ';')"
  if [ "$#" -eq 0 ]; then
    [ $rc -eq 0 ] || fail "$name should lint clean"
  else
    for r in "$@"; do echo "$got" | grep -q "VIOLATION $r " || fail "$name should violate $r"; done
  fi
}

say "# Cross-engine state-hash spike"
say ""
say "## A. Conforming guest, rules applied (lint clean + R3 NaN transform)"
expect_lint conforming
src=$(prep conforming nancanon)
run_cfgs conforming "$src"
cmp_ --expect equal --label "conforming: wasmtime vs wasmtime+canon vs wamr-interp vs wamr-aot" \
  wasmtime="$R/conforming.wt" wasmtime-canon="$R/conforming.wtc" wamr-interp="$R/conforming.wi" wamr-aot="$R/conforming.wa" || fail "A not equal"

say ""
say "## B. Counterexamples (each breaks one rule)"
say ""
say "### B1. R3 broken: NaN payloads left to the engine (same guest, no transform)"
src=$(prep conforming raw)
run_cfgs ce-nan "$src"
cmp_ --expect differ --label "R3 off: wasmtime vs wamr-interp vs wamr-aot" \
  wasmtime="$R/ce-nan.wt" wamr-interp="$R/ce-nan.wi" wamr-aot="$R/ce-nan.wa" || fail "B1a expected divergence"
cmp_ --expect differ --label "R3 off, wasmtime cranelift_nan_canonicalization=on vs wamr-interp (engine flag alone is not enough)" \
  wasmtime-canon="$R/ce-nan.wtc" wamr-interp="$R/ce-nan.wi" || fail "B1b expected divergence"

for ce in wasi-clock:R4 wasi-random:R4 relaxed-simd:R2 threads:R1; do
  name="${ce%%:*}"; rule="${ce##*:}"
  say ""
  say "### B. $name -> violates $rule"
  expect_lint "ce-$name" "$rule"
  src=$(prep "ce-$name" nancanon) # NaN transform on, so only the named rule can be the cause
  run_cfgs "ce-$name" "$src"
  case "$name" in
    wasi-clock|wasi-random)
      cmp_ --expect differ --label "$name: wasmtime run 1 vs run 2 (same engine, same inputs)" \
        run1="$R/ce-$name.wt" run2="$R/ce-$name.wt2" || fail "$name expected run-to-run divergence" ;;
    relaxed-simd)
      cmp_ --expect differ --label "relaxed-simd: wasmtime vs wamr-interp" \
        wasmtime="$R/ce-$name.wt" wamr-interp="$R/ce-$name.wi" || fail "$name expected divergence/load failure" ;;
    threads)
      # Atomics on a single thread are deterministic: the observable cost is engine support
      # (WAMR needs a build flag; Wasmtime needs threads enabled). Informational only.
      cmp_ --expect equal --label "threads (single-threaded atomics only): wasmtime vs wamr-interp vs wamr-aot" \
        wasmtime="$R/ce-$name.wt" wamr-interp="$R/ce-$name.wi" wamr-aot="$R/ce-$name.wa" || true ;;
  esac
done

say ""
if [ "$FAILS" -ne 0 ]; then say "RESULT: $FAILS expectation(s) violated"; exit 1; fi
say "RESULT: all expectations met"
