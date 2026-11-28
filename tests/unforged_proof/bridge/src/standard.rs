//! Existing STANDARD transport + strict namespace; synthetic-only, no gate authority.
use gecaos::caos::{CaosBaseSpec, CaosClient, CaosObjectKind, CaosTreeEntry, decode_caos_tree};
use serde_json::json;
use std::path::Path;

pub const WORKER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
salt=$(cat "$CAOS_ARGS_DIR/salt")
outer=$(readlink /proc/$$/ns/net)
root=$(mktemp -d "$TMPDIR/strict-step.XXXXXXXX")
mkdir -p "$root/cas/args" "$root/secrets" "$root/work" "$root/tmp"
cp -R "$CAOS_ARGS_DIR/state/." "$root/work/"
cp "$CAOS_ARGS_DIR/driver" "$root/cas/args/driver"
chmod 755 "$root/cas/args/driver"
mkdir "$root/listener"
"$root/cas/args/driver" canary-listen "$root/listener" "$salt" &
listener_pid=$!
trap 'kill "$listener_pid" 2>/dev/null || true; wait "$listener_pid" 2>/dev/null || true' EXIT
for ((try=0; try<100; try++)); do
  [ ! -f "$root/listener/ready" ] || break
  kill -0 "$listener_pid" || exit 125
  sleep 0.02
done
[ -f "$root/listener/ready" ] || exit 125
cp "$root/listener/ready" "$root/cas/args/canary-config"
"$root/cas/args/driver" canary-positive "$root/cas/args/canary-config"
baseline=$(cat "$root/listener/stats")
[ "$baseline" = '1 1 0' ] || exit 125
binary=${UNFORGED_UNFORGE_BIN:-}
if [ -z "$binary" ]; then binary=$(command -v unforge); fi
set +e
"$binary" caos-sandbox-exec --mode namespace --net proxy-only \
  --unforge-bin "$binary" --caos-bin "$(command -v caos)" \
  --cas-dir "$root/cas" --secret-dir "$root/secrets" \
  --work-dir "$root/work" --tmp-dir "$root/tmp" -- \
  bash -c 'set -euo pipefail; readlink /proc/$$/ns/net; if [ "${CAOS_SERVER_URL+x}" = x ]; then echo set; else echo unset; fi; if [ -e /proc/$$/fd/3 ]; then echo open; else echo closed; fi; /cas/args/driver canary-probe /cas/args/canary-config; cd /work; exec /cas/args/driver' \
  2> "$root/stderr" | {
    # Pipe bytes already consumed cannot be changed by the subsequent driver.
    # These outer files are outside every inner mount. Keep observations separate
    # from driver stdout and the ABI filesystem.
    IFS= read -r observed_net || observed_net=unavailable
    IFS= read -r observed_proxy || observed_proxy=unavailable
    IFS= read -r observed_fd3 || observed_fd3=unavailable
    IFS= read -r -n 128 observed_canary || observed_canary=unavailable
    printf '%s\n' "$observed_net" > "$root/netns"
    printf '%s\n' "$observed_proxy" > "$root/proxy"
    printf '%s\n' "$observed_fd3" > "$root/fd3"
    printf '%s\n' "$observed_canary" > "$root/canary"
    cat > "$root/stdout"
  }
statuses=("${PIPESTATUS[@]}")
code=${statuses[0]}
if [ "${statuses[1]}" != 0 ]; then code=125; fi
set -e
inner=$(cat "$root/netns" 2>/dev/null || printf unavailable)
proxy=$(cat "$root/proxy" 2>/dev/null || printf unavailable)
fd3=$(cat "$root/fd3" 2>/dev/null || printf unavailable)
canary=$(cat "$root/canary" 2>/dev/null || printf unavailable)
[[ "$canary" =~ ^(refused|timeout|unreachable),(refused|timeout|unreachable),(loopback-up|loopback-unavailable)$ ]] || { canary=unavailable; code=125; }
# Listener is outside all inner mounts. Stop and join before final counter read;
# no delayed accept can be hidden by reading counters while it is still active.
touch "$root/listener/stop"
if ! wait "$listener_pid"; then code=125; fi
trap - EXIT
post=$(cat "$root/listener/stats" 2>/dev/null || printf unavailable)
[ "$post" = '1 1 0' ] || code=125
[[ "$post" =~ ^[0-9]+\ [0-9]+\ [0-9]+$ ]] || post=unavailable
post=${post// /-}
[[ "$inner" =~ ^net:\[[0-9]+\]$ ]] || inner=unavailable
[[ "$proxy" = unset || "$proxy" = set ]] || proxy=unavailable
[[ "$fd3" = closed || "$fd3" = open ]] || fd3=unavailable
printf '{"request":"%s","salt":"%s","outer_netns":"%s","inner_netns":"%s","exit_code":%s,"caos_server_url_env":"%s","fd3":"%s","loopback_canary":"1-1-0:%s:%s"}\n' \
  "${CAOS_OWN_ARGTREE_HASH:-unset}" "$salt" "$outer" "$inner" "$code" "$proxy" "$fd3" "$canary" "$post" > "$root/diagnostics.json"
caos put "$root/diagnostics.json" /cas/out-trace
# Refuse rather than publish a partially executed ABI tree.
if [ "$code" != 0 ]; then cat "$root/stderr" >&2; exit "$code"; fi
caos put "$root/work" /cas/out
"#;

fn upload(client: &CaosClient, path: &Path, depth: usize, budget: &mut u64) -> Result<String, &'static str> {
    if depth > 32 { return Err("synthetic state depth exceeded"); }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|_| "state directory unavailable")? {
        let entry = entry.map_err(|_| "state entry unavailable")?;
        let name = entry.file_name().into_string().map_err(|_| "non UTF-8 state path")?;
        if name.is_empty() || name == ".git" || name.contains(['/', '\\', '\0']) { return Err("invalid state path"); }
        let meta = entry.file_type().map_err(|_| "state metadata unavailable")?;
        let (mode, oid) = if meta.is_dir() {
            ("40000", upload(client, &entry.path(), depth+1, budget)?)
        } else if meta.is_file() {
            let raw = read_bounded(&entry.path(), budget)?;
            ("100644", client.put_object(CaosObjectKind::Blob, &raw).map_err(|_| "state upload failed")?)
        } else { return Err("nonregular state entry"); };
        entries.push(CaosTreeEntry::new(mode, &name, &oid));
    }
    client.put_tree(&entries).map_err(|_| "state tree upload failed")
}

pub(crate) fn read_bounded(path: &Path, budget: &mut u64) -> Result<Vec<u8>, &'static str> {
    use std::io::Read;
    if !std::fs::symlink_metadata(path).map_err(|_| "input metadata unavailable")?.file_type().is_file() {
        return Err("input must be regular file");
    }
    let mut raw = Vec::new();
    // O_NOFOLLOW prevents replacement by a symlink between metadata/open.
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(0x20000)
        .open(path).map_err(|_| "input open failed")?;
    if !file.metadata().map_err(|_| "opened metadata unavailable")?.is_file() { return Err("opened input is not regular"); }
    file.take(*budget+1).read_to_end(&mut raw)
        .map_err(|_| "input read failed")?;
    if raw.len() as u64 > *budget { return Err("input budget exceeded"); }
    *budget -= raw.len() as u64;
    Ok(raw)
}

pub fn read_tree(client: &CaosClient, oid: &str, prefix: &str, depth: usize, budget: &mut u64) -> Result<(), &'static str> {
    if depth > 32 { return Err("output depth exceeded"); }
    let (kind, raw) = client.get_object(oid).map_err(|_| "output object unavailable")?;
    if raw.len() as u64 > *budget { return Err("output budget exceeded"); }
    *budget -= raw.len() as u64;
    if kind != CaosObjectKind::Tree { return Err("output is not tree"); }
    println!("{}", json!({"phase":"abi-object", "oid":oid,"kind":"tree", "hex":raw.iter().map(|b| format!("{b:02x}")).collect::<String>()}));
    for entry in decode_caos_tree(&raw).map_err(|_| "output tree invalid")? {
        if entry.name.is_empty() || matches!(entry.name.as_str(), "."|".."|".git") || entry.name.contains(['/', '\\', '\0']) {
            return Err("invalid output path");
        }
        let path = format!("{prefix}{}", entry.name);
        if entry.mode == "40000" {
            read_tree(client, &entry.oid, &format!("{path}/"), depth+1, budget)?;
        } else if entry.mode == "100644" {
            let (kind, raw) = client.get_object(&entry.oid).map_err(|_| "output blob unavailable")?;
            if kind != CaosObjectKind::Blob || raw.len() as u64 > *budget { return Err("invalid output blob"); }
            *budget -= raw.len() as u64;
            println!("{}", json!({"phase":"abi-object", "oid":entry.oid,"kind":"blob", "hex":raw.iter().map(|b| format!("{b:02x}")).collect::<String>()}));
            println!("{}", json!({"phase":"abi-file", "path":path, "mode":entry.mode, "hex":raw.iter().map(|b| format!("{b:02x}")).collect::<String>()}));
        } else { return Err("nonregular ABI output"); }
    }
    Ok(())
}

pub fn execute(client: &CaosClient, image: &str, salt: &str, driver: &Path, state: &Path) -> Result<(), &'static str> {
    let executable = read_bounded(driver, &mut (256*1024*1024))?;
    if !executable.starts_with(b"\x7fELF") { return Err("native ELF driver required"); }
    let state = upload(client, state, 0, &mut (64*1024*1024))?;
    let driver = client.put_object(CaosObjectKind::Blob, &executable).map_err(|_| "driver upload failed")?;
    let worker = client.put_object(CaosObjectKind::Blob, WORKER.as_bytes()).map_err(|_| "worker upload failed")?;
    let nonce = client.put_object(CaosObjectKind::Blob, salt.as_bytes()).map_err(|_| "salt upload failed")?;
    let pin = client.put_object(CaosObjectKind::Blob, image.as_bytes()).map_err(|_| "image pin upload failed")?;
    let args = client.put_tree(&[CaosTreeEntry::new("100755", "worker1", &worker),
        CaosTreeEntry::new("100755", "driver", &driver), CaosTreeEntry::new("40000", "state", &state),
        CaosTreeEntry::new("100644", "salt", &nonce), CaosTreeEntry::new("100644", "required-native-worker-image", &pin)])
        .map_err(|_| "arguments upload failed")?;
    let req = client.submit_arg_tree(&CaosBaseSpec::Unforged, &args, Some(salt)).map_err(|_| "submission failed")?;
    println!("{}", json!({"phase":"submitted", "request":req,"salt":salt,"args":args,"state":state,"driver":driver,"worker":worker,"expected_image":image}));
    let result = client.run_arg_tree_detailed(&req, true).map_err(|_| "strict recorded step failed")?;
    println!("{}", json!({"phase":"executed", "request":req,"result_oid":result.oid,"cache_hit":result.cache_hit,
        "worker_image":result.worker_image,"server_confirmed_nocache":result.server_confirmed_nocache}));
    if result.cache_hit || !result.server_confirmed_nocache || result.worker_image.as_deref() != Some(image) { return Err("fresh execution/image mismatch"); }
    crate::diagnostics::read_status(client, &req)?;
    read_tree(client, &result.oid, "", 0, &mut (64*1024*1024))?;
    Ok(())
}
