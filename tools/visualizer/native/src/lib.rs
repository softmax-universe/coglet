//! Reuse the existing offline native oracle, including fork applicability checks.
//! No providers, dispatchers, or live runtime bindings are constructed here.
// Keep one implementation shared with the CLI; the internal main is not invoked.
#[allow(dead_code)]
#[path = "main.rs"]
mod native;

use coglet_runtime::{NativeEngine, Runtime};
use serde_json::Value;

pub fn read_archive(path: &str) -> Result<Value, String> { native::read(path) }
pub fn parse_request(raw: &[u8]) -> Result<Value, String> { native::parse(raw) }

/// Independently verify an archive, then execute its recorded inputs afresh.
/// The returned archive contains genuinely executed output snapshots.
pub fn record(trace: &Value) -> Result<Value, String> {
    native::replay(trace)?;
    let setup = native::setup(trace)?;
    let mut runtime = Runtime::from_fixture(&setup)?;
    let code = hex::decode(setup["initial_files"]["/code/bin/step.wasm"].as_str().ok_or("code")?)
        .map_err(|e| e.to_string())?;
    let mut engine = NativeEngine::new(code, &setup["manifest"]["budgets"], 16_777_216, 262_144)?;
    let mut records = trace["records"].as_array().ok_or("records")?.clone();
    if !trace["trailing_boundary"].is_null() {
        records.push(trace["trailing_boundary"].clone());
    }
    // Runtime::replay verifies a cloned candidate and intentionally leaves its
    // receiver unchanged. Materialize a fresh archive with the same public step
    // and boundary operations, passing inputs only (never recorded outputs).
    for record in records {
        if record.get("step").is_some() {
            let mut input = serde_json::json!({});
            for key in ["trace_id", "coglet", "step", "logical_time", "seed", "engine", "budgets", "grants", "events", "previous", "boundary"] {
                input[key] = record[key].clone();
            }
            input["previous"] = runtime.records().last().map(|r| r["record_hash"].clone()).unwrap_or(Value::Null);
            let actual = runtime.step(&input, &mut engine)?;
            let mut actual = actual["journal"].clone();
            let mut expected = record.clone();
            // Independently regenerated wall observations change chain hashes.
            // Compare every deterministic journal field, not only the tree.
            for value in [&mut actual, &mut expected] {
                value.as_object_mut().ok_or("journal object")?.remove("record_hash");
                value.as_object_mut().unwrap().remove("previous");
                value["usage"].as_object_mut().ok_or("usage")?.remove("wall_time_us");
            }
            if actual != expected {
                return Err("reexecuted journal differs".into());
            }
        } else {
            runtime.seal_boundary(&record["trace_id"], record["logical_time"].as_u64().ok_or("clock")?, record["boundary"].as_array().ok_or("boundary")?)?;
        }
    }
    runtime.export_trace(&trace["records"][0]["trace_id"])
}

/// Reexecute a receipt-edited continuation through the visualizer's exact oracle.
pub fn fork_trace(trace: &Value, request: &Value) -> Result<Value, String> {
    native::fork_archive(trace, request)
}
