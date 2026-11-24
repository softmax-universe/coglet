//! Local supervisor tool, not a surface/hub operation or a production service.
use coglet_runtime::{NativeEngine, Runtime};
use serde_json::Value;
use std::io::{Read, Write};

pub fn run() -> Result<(), String> {
    const MAX: u64 = 32 * 1024 * 1024;
    let mut raw = Vec::new();
    std::io::stdin()
        .take(MAX + 1)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    if raw.len() as u64 > MAX {
        return Err("sandbox request exceeds 32 MiB".into());
    }
    let request: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let setup = &request["setup"];
    let mut runtime = Runtime::from_fixture(setup)?;
    let code = hex::decode(
        setup["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .ok_or("missing code")?,
    )
    .map_err(|e| e.to_string())?;
    let mut engine = NativeEngine::new(
        code,
        &setup["manifest"]["budgets"],
        16 * 1024 * 1024,
        64 * 1024,
    )?;
    let inputs = request["inputs"].as_array().ok_or("missing inputs")?;
    if inputs.is_empty() || inputs.len() > 1024 {
        return Err("sandbox requires 1..1024 steps".into());
    }
    for input in inputs {
        let mut input = input.clone();
        input["previous"] = runtime
            .records()
            .last()
            .map(|r| r["record_hash"].clone())
            .unwrap_or(Value::Null);
        runtime.step(&input, &mut engine)?;
    }
    let trace = runtime.export_trace(&inputs[0]["trace_id"])?;
    let raw = serde_json::to_vec(&trace).map_err(|e| e.to_string())?;
    if raw.len() > 64 * 1024 * 1024 {
        return Err("sandbox trace exceeds 64 MiB".into());
    }
    std::io::stdout().write_all(&raw).map_err(|e| e.to_string())
}
