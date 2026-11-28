//! STANDARD worker sidecar transport probe, NOT a replay or isolation certificate.
use gecaos::caos::{CaosBaseSpec, CaosClient, CaosObjectKind, CaosTreeEntry};
use serde_json::json;

// No model/provider/adapter/network canaries: these are actual process observations,
// not invented zero-call counters or proof that a configured policy is enforced.
const WORKER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
salt=$(cat "$CAOS_ARGS_DIR/salt")
netns=unavailable
if command -v readlink >/dev/null 2>&1; then
  netns=$(readlink /proc/self/ns/net) || netns=unavailable
fi
printf '{"request":"%s","salt":"%s","netns":"%s","sandbox_env":"%s","network_env":"%s"}\n' \
  "${CAOS_OWN_ARGTREE_HASH:-unset}" "$salt" "$netns" \
  "${UNFORGED_CAOS_SANDBOX_EFFECTIVE:-unset}" "${UNFORGED_CAOS_NET_EFFECTIVE:-unset}" > "$TMPDIR/diagnostics.json"
printf 'synthetic-sidecar-capability\n' > "$TMPDIR/result.txt"
caos put "$TMPDIR/diagnostics.json" /cas/out-trace
caos put "$TMPDIR/result.txt" /cas/out
"#;

pub fn probe(client: &CaosClient, expected_image: &str, salt: &str) -> Result<(), &'static str> {
    let worker = client.put_object(CaosObjectKind::Blob, WORKER.as_bytes())
        .map_err(|_| "diagnostic worker upload failed")?;
    let salt_oid = client.put_object(CaosObjectKind::Blob, salt.as_bytes())
        .map_err(|_| "diagnostic salt upload failed")?;
    let args = client.put_tree(&[
        CaosTreeEntry::new("100755", "worker1", &worker),
        CaosTreeEntry::new("100644", "salt", &salt_oid),
    ]).map_err(|_| "diagnostic argument upload failed")?;
    let req = client.submit_arg_tree(&CaosBaseSpec::Unforged, &args, Some(salt))
        .map_err(|_| "diagnostic submission failed")?;
    // Record identity before execution so even infrastructure refusal is retained.
    println!("{}", json!({"phase":"submitted", "request":req, "worker":worker,
        "args":args, "salt":salt, "expected_image":expected_image,
        "qualification":"synthetic diagnostics transport only; NOT stage2 PASS"}));
    let result = client.run_arg_tree_detailed(&req, true)
        .map_err(|_| "standard diagnostic execution failed (raw diagnostics suppressed)")?;
    println!("{}", json!({"phase":"executed", "request":req,
        "result_oid":result.oid, "cache_hit":result.cache_hit,
        "server_confirmed_nocache":result.server_confirmed_nocache,
        "worker_image":result.worker_image}));
    if result.cache_hit || !result.server_confirmed_nocache
        || result.worker_image.as_deref() != Some(expected_image) {
        return Err("diagnostic cold execution or image identity mismatch");
    }
    let status = client.request_with_headers("GET", &format!("/status/{req}?all=1"), None, None)
        .map_err(|_| "diagnostic status readback failed")?;
    // Preserve the actual status before interpretation. No candidate input is used.
    let value: serde_json::Value = serde_json::from_slice(&status.body)
        .map_err(|_| "diagnostic status is not JSON")?;
    println!("{}", json!({"phase":"status", "request":req,
        "http_status":status.status, "body":value}));
    if status.status != 200 || value.get("arg_tree").and_then(|v| v.as_str()) != Some(&req) {
        return Err("diagnostic status does not name submitted request");
    }
    let traces = value.get("out_trace").and_then(|v| v.as_array())
        .ok_or("diagnostic sidecar missing from completed status")?;
    if traces.len() != 1 { return Err("diagnostic sidecar missing or ambiguous"); }
    let oid = traces[0].as_str().filter(|s| s.len() == 40
        && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .ok_or("diagnostic sidecar identity invalid")?;
    let (kind, payload) = client.get_object(oid)
        .map_err(|_| "diagnostic object readback failed")?;
    if kind != CaosObjectKind::Blob { return Err("diagnostic sidecar is not a blob"); }
    let diagnostic: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|_| "diagnostic object is not JSON")?;
    if diagnostic.get("request").and_then(|v| v.as_str()) != Some(&req)
        || diagnostic.get("salt").and_then(|v| v.as_str()) != Some(salt) {
        return Err("diagnostic object request/salt mismatch");
    }
    println!("{}", json!({"phase":"sidecar", "request":req, "oid":oid,
        "observations":diagnostic,
        "qualification":"actual worker observations, NOT isolation or no-live-effects proof"}));
    Ok(())
}
