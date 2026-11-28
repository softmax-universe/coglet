//! Synthetic provider outcome, genuine native guest execution. No provider is linked.
use coglet_runtime::{NativeEngine, Runtime, cbor, digest};
use serde_json::{Value, json};
use std::path::Path;

pub fn record(root: &Path) -> Result<Value, String> {
    let load = |p: &str| {
        coglet_visualizer_native::read_archive(
            root.join(p).to_str().ok_or("non-UTF8 fixture root")?,
        )
    };
    let cases = load("conformance/abi/cases.json")?;
    let case = cases
        .as_array()
        .ok_or("cases array")?
        .iter()
        .find(|c| c["name"] == "async-llm-two-wakes")
        .ok_or("model case")?;
    let sends: Vec<_> = case["steps"]
        .as_array()
        .ok_or("steps")?
        .iter()
        .filter_map(|s| s.get("send"))
        .collect();
    let archives = load("conformance/abi/executable-archive.json")?;
    let trace = &archives
        .as_array()
        .ok_or("archives")?
        .iter()
        .find(|c| c["name"] == "executable-provenance-control")
        .ok_or("control")?["steps"][0]["send"]["trace"];
    let mut setup = sends[0].clone();
    // Never use guest_script, calls, expected output, or expected journals.
    setup.as_object_mut().ok_or("setup")?.remove("guest_script");
    setup["manifest"]["budgets"] = trace["records"][0]["budgets"].clone();
    setup["manifest"]["budgets"]["wall_time_us"] = json!(5_000_000);
    let request = json!({"id":"4".repeat(32),"correlation_id":"4".repeat(32),
        "capability":"llm.complete","model":"small","input":{"prompt":"hello"}});
    let raw = cbor(&request)?;
    let data: String = raw.iter().map(|b| format!("\\{b:02x}")).collect();
    // The guest has independent scenario semantics: request once, then return.
    // A later increment may inspect reply content; no recorded result is embedded.
    let code = wat::parse_str(format!(
        r#"(module
        (import "coglet_v0" "llm" (func $llm (param i32 i32) (result i32)))
        (memory (export "memory") 1 2)
        (data (i32.const 0) "{data}")
        (func (export "abi_alloc") (param i32) (result i32) i32.const 8192)
        (func (export "handle") (param i32 i32) (param $step i64) (param i64 i64) (result i32)
          local.get $step i64.eqz if i32.const 0 i32.const {} call $llm drop end
          i32.const 0))"#,
        raw.len()
    ))
    .map_err(|e| e.to_string())?;
    let provenance_bytes = hex::decode(
        trace["initial_files"]["/code/build/provenance.cbor"]
            .as_str()
            .ok_or("provenance")?,
    )
    .map_err(|e| e.to_string())?;
    let mut provenance: Value = coglet_types::decode("hub-build-record", &provenance_bytes)?
        .deserialized()
        .map_err(|e| e.to_string())?;
    provenance["rewritten_sha256"] = json!(digest(&code));
    setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
    setup["initial_files"]["/code/build/provenance.cbor"] = json!(hex::encode(cbor(&provenance)?));
    let mut runtime = Runtime::from_fixture(&setup)?;
    let mut engine = NativeEngine::new(code, &setup["manifest"]["budgets"], 16_777_216, 65_536)?;
    let mut previous = Value::Null;
    for send in sends {
        let mut input = send["inputs"].clone();
        input["budgets"] = setup["manifest"]["budgets"].clone();
        input["previous"] = previous;
        let result = runtime.step(&input, &mut engine)?;
        if result["journal"]["status"] != "committed" {
            return Err(format!("model fixture step failed: {result}"));
        }
        previous = result["record_hash"].clone();
    }
    let archive = runtime.export_trace(&setup["inputs"]["trace_id"])?;
    // Independent native replay verifies recorded request/reply applicability.
    coglet_visualizer_native::record(&archive)?;
    Ok(archive)
}
