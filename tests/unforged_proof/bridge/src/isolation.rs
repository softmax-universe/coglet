//! Existing strict nested sandbox route. Not a complete no-live-effects proof.
pub const WORKER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
salt=$(cat "$CAOS_ARGS_DIR/salt")
outer=$(readlink /proc/$$/ns/net) || outer=unavailable
root="$TMPDIR/strict-inner"
mkdir -p "$root/cas/args" "$root/secrets" "$root/work" "$root/tmp"
binary=${UNFORGED_UNFORGE_BIN:-}
if [ -z "$binary" ]; then binary=$(command -v unforge || true); fi
# Explicit namespace: never use Auto/Worktree or pass the outer capability socket.
# stdin/stdout/stderr are ordinary observations; fd3 is not preserved.
set +e
"$binary" caos-sandbox-exec --mode namespace --net proxy-only \
  --unforge-bin "$binary" --caos-bin "$(command -v caos)" \
  --cas-dir "$root/cas" --secret-dir "$root/secrets" \
  --work-dir "$root/work" --tmp-dir "$root/tmp" -- \
  bash -c 'readlink /proc/$$/ns/net; if [ "${CAOS_SERVER_URL+x}" = x ]; then echo set; else echo unset; fi; if [ -e /proc/$$/fd/3 ]; then echo open; else echo closed; fi' \
  > "$root/stdout" 2> "$root/stderr"
code=$?
set -e
mapfile -t observations < "$root/stdout"
inner=${observations[0]:-unavailable}
proxy=${observations[1]:-unavailable}
fd3=${observations[2]:-unavailable}
# Strictly allowlisted strings only; arbitrary error text is a separate raw blob.
[[ "$inner" =~ ^net:\[[0-9]+\]$ ]] || inner=unavailable
[[ "$proxy" = unset || "$proxy" = set ]] || proxy=unavailable
[[ "$fd3" = closed || "$fd3" = open ]] || fd3=unavailable
printf '{"request":"%s","salt":"%s","outer_netns":"%s","inner_netns":"%s","exit_code":%s,"caos_server_url_env":"%s","fd3":"%s"}\n' \
  "${CAOS_OWN_ARGTREE_HASH:-unset}" "$salt" "$outer" "$inner" "$code" "$proxy" "$fd3" > "$root/diagnostics.json"
# Retain the actual failure instead of succeeding by fallback or forging counters.
cat "$root/stdout" "$root/stderr" > "$root/result.txt"
caos put "$root/diagnostics.json" /cas/out-trace
caos put "$root/result.txt" /cas/out
"#;
