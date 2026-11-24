//! Offline native verifier. No dispatch, provider access, or observed-output engine.
mod strict;
use coglet_runtime::{NativeEngine, Runtime, RuntimeService};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};
type Result<T> = std::result::Result<T, String>;

fn read(path: &str) -> Result<Value> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let source = options.open(path).map_err(|e| e.to_string())?;
    let metadata = source.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
        return Err("archive must be a regular file up to 128 MiB".into());
    }
    let mut raw = Vec::new();
    source
        .take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    if raw.len() > 128 * 1024 * 1024 {
        return Err("archive exceeds 128 MiB".into());
    }
    strict::parse(&raw)
}
fn setup(trace: &Value) -> Result<Value> {
    let pin = trace["manifest_hash"].as_str().ok_or("manifest pin")?;
    let raw = hex::decode(trace["blobs"][pin].as_str().ok_or("manifest blob")?)
        .map_err(|e| e.to_string())?;
    let manifest: Value = coglet_types::decode("manifest", &raw)?
        .deserialized()
        .map_err(|e| e.to_string())?;
    let mut value = json!({"manifest": manifest});
    for key in [
        "initial_files",
        "initial_effect_ids",
        "initial_actions",
        "initial_inbox_wakes",
        "initial_timers",
        "initial_llm_requests",
    ] {
        value[key] = trace
            .get(key)
            .ok_or_else(|| format!("missing {key}"))?
            .clone();
    }
    Ok(value)
}
fn engine(setup: &Value) -> Result<NativeEngine> {
    let code = hex::decode(
        setup["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .ok_or("missing code")?,
    )
    .map_err(|e| e.to_string())?;
    NativeEngine::new(code, &setup["manifest"]["budgets"], 16_777_216, 262_144)
}
fn project(record: &Value) -> Value {
    let mut value = json!({});
    for key in [
        "version",
        "trace_id",
        "coglet",
        "step",
        "logical_time",
        "status",
        "output_hash",
        "record_hash",
        "events",
    ] {
        value[key] = record[key].clone();
    }
    value["output_files"] = Value::Object(
        record["output_files"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("/data/"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    value
}
fn replay(trace: &Value) -> Result<Value> {
    // Structural import is NOT guest verification. Run an independent engine
    // from the complete initial setup; never execute the imported receiver.
    Runtime::import_trace(trace)?;
    let setup = setup(trace)?;
    let records = trace["records"].as_array().ok_or("records")?;
    if records.is_empty() || records.len() > 128 {
        return Err("require 1–128 guest steps".into());
    }
    let mut all = records.clone();
    if !trace["trailing_boundary"].is_null() {
        all.push(trace["trailing_boundary"].clone());
    }
    let hashes = Runtime::from_fixture(&setup)?.replay(&all, &mut engine(&setup)?)?;
    if hashes.last().map(String::as_str) != trace["final_hash"].as_str() {
        return Err("native final hash mismatch".into());
    }
    Ok(
        json!({"verified_native":true,"final_hash":trace["final_hash"],"hashes":hashes,"records":records.iter().map(project).collect::<Vec<_>>()}),
    )
}
fn fork(trace: &Value, request: &Value, baseline: &Value) -> Result<Value> {
    let object = request
        .as_object()
        .ok_or("fork request must be an object")?;
    if object.len() != 2 || !object.contains_key("receipt_index") || !object.contains_key("result")
    {
        return Err("unsupported replay request".into());
    }
    let target = request["receipt_index"].as_u64().ok_or("receipt index")?;
    if !request["result"].is_object() {
        return Err("receipt result must be an object".into());
    }
    let setup = setup(trace)?;
    let mut runtime = Runtime::from_fixture(&setup)?;
    let mut engine = engine(&setup)?;
    let mut files = setup["initial_files"]
        .as_object()
        .ok_or("initial files")?
        .clone();
    let mut previous = Value::Null;
    let mut count = 0u64;
    let mut edited = false;
    let mut hashes = Vec::new();
    let mut projected = Vec::new();
    let mut divergence = None;
    let mut originals = trace["records"].as_array().ok_or("records")?.clone();
    if !trace["trailing_boundary"].is_null() {
        originals.push(trace["trailing_boundary"].clone());
    }
    for (index, original) in originals.iter().enumerate() {
        let mut boundary = original["boundary"].as_array().ok_or("boundary")?.clone();
        // Scratch state tracks ordered changes within this boundary. Only native
        // step/seal commits these bytes; recorded output snapshots are never used.
        let mut scratch = files.clone();
        for change in &mut boundary {
            let path = change["path"].as_str().ok_or("boundary path")?.to_owned();
            let current = scratch.get(&path).cloned().unwrap_or(Value::Null);
            if path == "/data/inbox.jsonl" {
                let old = change["before"].as_str().unwrap_or("");
                let after = change["after"].as_str().ok_or("inbox after")?;
                let suffix = after.strip_prefix(old).ok_or("inbox append prefix")?;
                let mut bytes = hex::decode(suffix).map_err(|e| e.to_string())?;
                if change["method"] == "runtime.receipt" {
                    let mut message: Value =
                        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                    if message["event"]["kind"] == "action.outcome"
                        && message["event"]["payload"]["status"] == "succeeded"
                    {
                        if count == target {
                            message["event"]["payload"]["result"] = request["result"].clone();
                            bytes = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
                            bytes.push(b'\n');
                            edited = true;
                        }
                        count += 1;
                    }
                }
                change["before"] = current.clone();
                change["after"] = json!(format!(
                    "{}{}",
                    current.as_str().unwrap_or(""),
                    hex::encode(bytes)
                ));
            } else if current != change["before"] {
                return Err("fork inapplicable: non-inbox boundary no longer matches".into());
            }
            if change["after"].is_null() {
                scratch.remove(&path);
            } else {
                scratch.insert(path, change["after"].clone());
            }
        }
        let actual = if original.get("step").is_some() {
            let mut inputs = json!({});
            for key in [
                "trace_id",
                "coglet",
                "step",
                "logical_time",
                "seed",
                "engine",
                "budgets",
                "grants",
                "events",
            ] {
                inputs[key] = original[key].clone();
            }
            inputs["previous"] = previous.clone();
            inputs["boundary"] = json!(boundary);
            let journal = runtime.step(&inputs, &mut engine)?["journal"].clone();
            for key in ["actions", "llm_requests", "timers", "status", "failure"] {
                if journal[key] != original[key] {
                    return Err(format!(
                        "fork inapplicable: generated {key} changed; recorded external continuation is not reusable"
                    ));
                }
            }
            projected.push(project(&journal));
            journal
        } else {
            runtime.seal_boundary(
                &original["trace_id"],
                original["logical_time"].as_u64().ok_or("clock")?,
                &boundary,
            )?
        };
        previous = actual["record_hash"].clone();
        files = actual["output_files"]
            .as_object()
            .ok_or("native files")?
            .clone();
        if divergence.is_none() && actual["output_hash"] != original["output_hash"] {
            divergence = Some(index);
        }
        hashes.push(actual["output_hash"].clone());
    }
    if !edited {
        return Err("receipt index not found".into());
    }
    Ok(
        json!({"verified_native":true,"baseline_final_hash":baseline["final_hash"],
        "final_hash":hashes.last(),"hashes":hashes,"first_divergence":divergence,"records":projected,
        "qualification":"Native fork with edited external receipt result. All other recorded actions, timers and LLM requests must match exactly; no live effects or providers."}),
    )
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("export") if args.len() == 4 => {
            // Only an explicitly selected STOPPED TEST WAL, never a live router.
            let setup = read(&args[1])?;
            let mut service = RuntimeService::open(Path::new(&args[2]), &setup, 128)?;
            let last = service.records().last().ok_or("empty capture")?;
            let id = last["trace_id"].clone();
            let time = last["logical_time"].as_u64().ok_or("clock")?;
            let trace = service.export_trace(&id, time)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&args[3]).map_err(|e| e.to_string())?;
            file.write_all(&serde_json::to_vec(&trace).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        Some("replay") if args.len() == 2 => {
            let mut request = String::new();
            io::stdin()
                .take(65_537)
                .read_to_string(&mut request)
                .map_err(|e| e.to_string())?;
            if request.len() > 65_536 {
                return Err("request too large".into());
            }
            let request = strict::parse(request.as_bytes())?;
            let trace = read(&args[1])?;
            let baseline = replay(&trace)?;
            println!(
                "{}",
                if request == json!({}) {
                    baseline
                } else {
                    fork(&trace, &request, &baseline)?
                }
            );
        }
        _ => {
            return Err(
                "usage: export SETUP STOPPED_TEST_WAL NEW_TRACE | replay TRACE < REQUEST".into(),
            );
        }
    }
    Ok(())
}
