//! Offline WAL reader and independent WAMR replay, never recorded-output injection.
use coglet_runtime::{NativeEngine, Runtime, RuntimeService, cbor, digest};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn replay(setup: &Value, records: &[Value]) -> Result<Vec<String>, String> {
    let wasm = hex::decode(
        setup["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let mut engine = NativeEngine::new(wasm, &setup["manifest"]["budgets"], 16_777_216, 262_144)?;
    Runtime::from_fixture(setup)?.replay(records, &mut engine)
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 && args.len() != 4 {
        return Err("usage: proof SETUP WAL OUTPUT [ARCHIVE]".into());
    }
    let setup: Value = serde_json::from_slice(&fs::read(&args[0]).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut service = RuntimeService::open(Path::new(&args[1]), &setup, 128)?;
    let records = service.records().to_vec();
    if records.is_empty() {
        return Err("no guest steps committed".into());
    }
    for record in &records {
        if record["status"] != "committed" || record["usage"]["compute"].as_u64().unwrap_or(0) == 0
        {
            return Err(format!("not an executed committed guest step: {record}"));
        }
        if record["engine"]["build"] != coglet_engine_wamr::BUILD {
            return Err("not the pinned real WAMR engine".into());
        }
    }
    let hashes = replay(&setup, &records)?;
    let expected: Vec<_> = records
        .iter()
        .map(|r| r["output_hash"].as_str().unwrap())
        .collect();
    if hashes != expected {
        return Err("live/replay state hashes differ".into());
    }
    // Valid-checksum altered seed must still fail independent re-execution.
    let mut altered = records.clone();
    altered[0]["seed"] = json!(altered[0]["seed"].as_u64().unwrap() ^ 1);
    altered[0].as_object_mut().unwrap().remove("record_hash");
    altered[0]["record_hash"] = json!(digest(&cbor(&altered[0])?));
    if replay(&setup, &altered[..1]).is_ok() {
        return Err("changed seed accepted".into());
    }
    // A differently pinned executable cannot reproduce this archive, even if
    // changing only an inert custom section. This is the code-identity control.
    let mut changed = setup.clone();
    let mut wasm = hex::decode(
        changed["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    wasm.extend_from_slice(&[0, 2, 1, b'x']);
    changed["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(wasm));
    if replay(&changed, &records).is_ok() {
        return Err("changed guest accepted".into());
    }
    if let Some(destination) = args.get(3) {
        // Reuse the complete existing archive exporter, including any sealed
        // trailing boundary. Local fixture acquisition is not an export grant.
        let last = records.last().ok_or("no records")?;
        let archive = service.export_trace(
            &last["trace_id"], last["logical_time"].as_u64().ok_or("logical time")?)?;
        fs::write(destination, serde_json::to_vec(&archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    fs::write(
        &args[2],
        serde_json::to_vec_pretty(&json!({
            "records":records,"state_hashes":hashes,"independent_guest_reexecution":true,
            "changed_seed_rejected":true,"changed_guest_rejected":true
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
