//! ADR-0015 corpus builder. Scenario code never reads recorded calls/results/files.
use coglet_runtime::{NativeEngine, Runtime, cbor, digest};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
pub fn sources() -> Vec<(String, Value)> {
    ["boundary-receipts", "provenance", "heard-archive"]
        .into_iter()
        .flat_map(|p| {
            let cases: Vec<Value> = serde_json::from_slice(
                &std::fs::read(root().join(format!("conformance/abi/{p}.json"))).unwrap(),
            )
            .unwrap();
            cases.into_iter().map(move |c| (p.to_owned(), c))
        })
        .collect()
}
pub fn valid(case: &Value) -> bool {
    case["steps"][1]
        .get("expect")
        .or_else(|| case["steps"][1].get("match"))
        .unwrap()["valid"]
        == true
}
pub fn setup(trace: &Value) -> Value {
    let raw = hex::decode(
        trace["blobs"][trace["manifest_hash"].as_str().unwrap()]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let manifest: Value = coglet_types::decode("manifest", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    let mut s = json!({"manifest": manifest});
    for k in [
        "initial_files",
        "initial_effect_ids",
        "initial_actions",
        "initial_inbox_wakes",
        "initial_timers",
        "initial_llm_requests",
    ] {
        s[k] = trace[k].clone();
    }
    s
}
pub fn engine(trace: &Value) -> NativeEngine {
    let s = setup(trace);
    NativeEngine::new(
        hex::decode(s["initial_files"]["/code/bin/step.wasm"].as_str().unwrap()).unwrap(),
        &s["manifest"]["budgets"],
        8_388_608,
        65_536,
    )
    .unwrap()
}
/// Hand-authored scenario semantics, not a compiler for recorded host calls.
pub fn program(name: &str) -> String {
    let emit = matches!(
        name,
        "terminal-guest-rejection-after-commit"
            | "terminal-guest-outcome-after-commit"
            | "terminal-rejection-prestep-delivery"
            | "terminal-rejection-prestep-abort"
            | "terminal-outcome-prestep-delivery"
            | "terminal-outcome-prestep-abort"
            | "terminal-after-aborted-emit"
    );
    let abort_first = matches!(
        name,
        "trailing-write-after-abort"
            | "terminal-rejection-wake-abort"
            | "terminal-outcome-wake-abort"
            | "terminal-after-aborted-emit"
    );
    let abort_second = matches!(
        name,
        "terminal-rejection-prestep-abort" | "terminal-outcome-prestep-abort"
    );
    // Independently specified motor intent; no expected return is embedded.
    let action = json!({"id":"4".repeat(32),"coglet":format!("coglet:{}", "1".repeat(64)),"capability":"motor.move","args":{"x":1},"deadline":100,"correlation_id":"4".repeat(32)});
    let raw = cbor(&action).unwrap();
    let data: String = raw.iter().map(|b| format!("\\{b:02x}")).collect();
    let call = if emit {
        format!(
            "local.get $step i64.eqz if i32.const 0 i32.const {} call $emit drop end",
            raw.len()
        )
    } else {
        String::new()
    };
    let failure = if abort_first {
        "local.get $step i64.eqz if i32.const 1 return end"
    } else if abort_second {
        "local.get $step i64.const 1 i64.eq if i32.const 1 return end"
    } else {
        ""
    };
    format!(
        "(module\n (import \"coglet_v0\" \"emit\" (func $emit (param i32 i32) (result i32)))\n (memory (export \"memory\") 1 2)\n (data (i32.const 0) \"{data}\")\n (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)\n (func (export \"handle\") (param i32 i32) (param $step i64) (param i64 i64) (result i32)\n {call}\n {failure}\n i32.const 0))\n"
    )
}
pub fn rejection_reason(name: &str) -> &'static str {
    match name {
        "trailing-wrong-previous" => "trailing_boundary.previous changed",
        "trailing-time-regression" => "trailing_boundary.logical_time regresses",
        "trailing-omitted-with-final-change" => {
            "trailing_boundary omitted despite final_hash mutation"
        }
        "trailing-unattributed-mutation" => {
            "trailing_boundary.output_files mutated without boundary attribution"
        }
        "terminal-after-aborted-emit" => {
            "trailing_boundary contains a terminal receipt for an uncommitted emitted action; baseline omits impossible receipt"
        }
        "heard-guest-write" => {
            "records[0].output_files changes reserved heard state after legitimate runtime.heard boundary; baseline guest performs no write"
        }
        "heard-guest-create" => {
            "records[0].output_files creates reserved heard state without a runtime boundary; baseline guest performs no write"
        }
        "provenance-missing" => "initial_files omits /code/build/provenance.cbor",
        "provenance-wrong-output" => "provenance.rewritten_sha256 differs from pinned code digest",
        "provenance-missing-policy" => "provenance.compute_policy omitted",
        "provenance-unknown-field" => "provenance contains unknown field",
        "provenance-malformed-cbor" => "provenance contains malformed CBOR",
        "provenance-trailing-cbor" => "provenance contains trailing CBOR item",
        "provenance-noncanonical-cbor" => "provenance contains non-core-deterministic CBOR",
        _ => "archive-level rejection; no purported guest transition",
    }
}
pub fn build() -> (Value, Vec<Value>) {
    let all = sources();
    let control = &all
        .iter()
        .find(|(_, c)| c["name"] == "provenance-control")
        .unwrap()
        .1["steps"][0]["send"]["trace"];
    let control_setup = setup(control);
    let mut inventory = vec![];
    let mut cases = vec![];
    for (profile, source) in &all {
        let name = source["name"].as_str().unwrap();
        let t = &source["steps"][0]["send"]["trace"];
        let count = t["records"].as_array().unwrap().len();
        let accepted = valid(source);
        let mut entry = json!({"profile":profile,"source":name,"source_sha256":digest(&cbor(source).unwrap()),"guest_records":[],"boundary":"runtime validation only","rejection":if accepted {Value::Null} else {json!(rejection_reason(name))}});
        if accepted || count > 0 {
            let wat = program(name);
            let code = wat::parse_str(&wat).unwrap();
            let mut s = setup(t);
            let mut provenance: Value = coglet_types::decode(
                "hub-build-record",
                &hex::decode(
                    control_setup["initial_files"]["/code/build/provenance.cbor"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
            .deserialized()
            .unwrap();
            provenance["rewritten_sha256"] = json!(digest(&code));
            s["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
            s["initial_files"]["/code/build/provenance.cbor"] =
                json!(hex::encode(cbor(&provenance).unwrap()));
            s["manifest"]["budgets"]["compute_mode"] = json!(coglet_engine_wamr::COMPUTE_MODE);
            s["manifest"]["budgets"]["compute"] = json!(100_000);
            s["manifest"]["budgets"]["memory_pages"] = json!(2);
            s["manifest"]["budgets"]["host_bytes"] = json!(1_000_000);
            s["manifest"]["budgets"]["wall_time_us"] = json!(5_000_000);
            let mut runtime = Runtime::from_fixture(&s).unwrap();
            let mut native =
                NativeEngine::new(code.clone(), &s["manifest"]["budgets"], 8_388_608, 65_536)
                    .unwrap();
            for (index, r) in t["records"].as_array().unwrap().iter().enumerate() {
                let mut inputs = json!({});
                for k in [
                    "trace_id",
                    "coglet",
                    "step",
                    "logical_time",
                    "seed",
                    "grants",
                    "events",
                    "boundary",
                ] {
                    inputs[k] = r[k].clone();
                }
                inputs["budgets"] = s["manifest"]["budgets"].clone();
                inputs["previous"] = runtime
                    .records()
                    .last()
                    .map(|r| r["record_hash"].clone())
                    .unwrap_or(Value::Null);
                runtime
                    .step(&inputs, &mut native)
                    .unwrap_or_else(|e| panic!("{name} step {index}: {e}"));
                entry["guest_records"].as_array_mut().unwrap().push(json!({"source_record":index,"companion":format!("executable-{name}"),"companion_record":index}));
            }
            // Invalid trailing data is never executed as if it were a valid input.
            if accepted && !t["trailing_boundary"].is_null() {
                let b = &t["trailing_boundary"];
                runtime
                    .seal_boundary(
                        &t["trace_id"],
                        b["logical_time"].as_u64().unwrap(),
                        b["boundary"].as_array().unwrap(),
                    )
                    .unwrap();
            }
            let trace = runtime.export_trace(&t["trace_id"]).unwrap();
            Runtime::import_trace(&trace).unwrap();
            cases.push(json!({"name":format!("executable-{name}"),"steps":[{"send":{"operation":"abi.validate_trace","trace":trace}},{"match":{"valid":true,"dispatched":false,"engine_invocations":0},"within_ms":1000}]}));
            entry["companion"] = json!(format!("executable-{name}"));
            entry["wat"] = json!(wat);
            entry["wasm_hex"] = json!(hex::encode(code));
        }
        entry["runtime_boundaries"] = json!({"pre_step": t["records"].as_array().unwrap().iter().enumerate().filter(|(_, r)| !r["boundary"].as_array().unwrap().is_empty()).map(|(i, _)| i).collect::<Vec<_>>(), "trailing": !t["trailing_boundary"].is_null()});
        inventory.push(entry);
    }
    (json!(inventory), cases)
}
