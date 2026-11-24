//! ABI 0.2 archive export/import of exact lossless DB records, and the daemon
//! admission boundary. Offline archive validation is not guest replay.
use coglet_runtime::{MockEngine, Runtime, admit_manifest, cbor, digest};
use serde_json::{Value, json};

const CATALOG: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn send() -> Value {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    cases.iter().find(|c| c["name"] == "no-effects").unwrap()["steps"][0]["send"].clone()
}

fn database_setup() -> Value {
    let mut setup = send();
    let mut manifest = setup["manifest"].clone();
    manifest["version"] = json!("0.2");
    manifest["database"] = json!({"schema_min":1,"schema_max":1,"catalog_hash":CATALOG});
    setup["manifest"] = manifest;
    setup
}

fn db_call() -> Value {
    json!({"op":"db","argument":{"operation":"statement","statement":"scan","mode":"read",
        "params":[],"max_rows":64,"max_bytes":65536},"result":0,
        "response":{"status":"page","columns":["id"],"rows":[],"affected":0,"revision":3,"next_cursor":null}})
}

fn exact_record(mut record: Value) -> (Value, Vec<u8>) {
    record["version"] = json!("0.2");
    record["db_before"] = json!(3);
    record["db_after"] = json!(3);
    record["db_schema_version"] = json!(1);
    record["db_catalog_hash"] = json!(CATALOG);
    let mut calls = record["calls"].as_array().cloned().unwrap_or_default();
    calls.insert(0, db_call());
    record["calls"] = json!(calls);
    record.as_object_mut().unwrap().remove("record_hash");
    let hash = digest(&cbor(&record).unwrap());
    record["record_hash"] = json!(hash);
    let native = coglet_types::decode_profile(&cbor(&record).unwrap()).unwrap();
    let raw = coglet_types::encode("database::db-step-record", &native).unwrap();
    let projection: Value = coglet_types::decode("database::db-step-record", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    (projection, raw)
}

#[test]
fn empty_database_archive_roundtrips_without_guest_execution() {
    let setup = database_setup();
    let runtime = Runtime::from_fixture(&setup).unwrap();
    let id = &setup["inputs"]["trace_id"];
    let trace = runtime.export_trace(id).unwrap();
    assert_eq!(trace["version"], "0.2");
    assert!(trace["records"].as_array().unwrap().is_empty());
    assert_eq!(trace["initial_db_revision"], 0);
    assert_eq!(trace["final_db_revision"], 0);
    let imported = Runtime::import_trace(&trace).unwrap();
    assert_eq!(imported.export_trace(id).unwrap(), trace);
    assert_eq!(
        imported.archive_state().unwrap(),
        runtime.archive_state().unwrap()
    );
}

#[test]
fn database_archive_accepts_exact_lossless_records_and_rejects_projection_only() {
    let setup = database_setup();
    let mut live = Runtime::from_fixture(&setup).unwrap();
    live.step(
        &setup["inputs"],
        &mut MockEngine::new(setup["guest_script"].clone()),
    )
    .unwrap();
    let (record, raw) = exact_record(live.records()[0].clone());
    let mut header = Runtime::from_fixture(&setup)
        .unwrap()
        .export_trace(&setup["inputs"]["trace_id"])
        .unwrap();
    header["records"] = json!([record.clone()]);
    header["initial_db_revision"] = json!(3);
    header["final_db_revision"] = json!(3);
    assert!(
        Runtime::import_trace(&header).is_err(),
        "projection-only ABI 0.2 archive imported"
    );
    header["blobs"][digest(&raw)] = json!(hex::encode(&raw));
    let imported = Runtime::import_trace(&header).unwrap();
    let exported = imported.export_trace(&setup["inputs"]["trace_id"]).unwrap();
    assert_eq!(exported["version"], "0.2");
    assert_eq!(exported["records"][0]["calls"][0]["op"], "db");
    assert_eq!(
        exported["blobs"][digest(&raw)].as_str().unwrap(),
        hex::encode(&raw)
    );
    assert_eq!(
        Runtime::import_trace(&exported)
            .unwrap()
            .export_trace(&exported["trace_id"])
            .unwrap(),
        exported
    );
}

#[test]
fn database_archive_rejects_tampered_exact_record_bytes() {
    let setup = database_setup();
    let mut live = Runtime::from_fixture(&setup).unwrap();
    live.step(
        &setup["inputs"],
        &mut MockEngine::new(setup["guest_script"].clone()),
    )
    .unwrap();
    let (record, mut raw) = exact_record(live.records()[0].clone());
    let mut header = Runtime::from_fixture(&setup)
        .unwrap()
        .export_trace(&setup["inputs"]["trace_id"])
        .unwrap();
    header["records"] = json!([record]);
    header["initial_db_revision"] = json!(3);
    header["final_db_revision"] = json!(3);
    *raw.last_mut().unwrap() ^= 1;
    header["blobs"][digest(&raw)] = json!(hex::encode(&raw));
    assert!(
        Runtime::import_trace(&header).is_err(),
        "tampered exact database record imported"
    );
}

#[test]
fn mixed_ordinary_records_on_db_manifest_do_not_export() {
    let setup = database_setup();
    let mut live = Runtime::from_fixture(&setup).unwrap();
    live.step(
        &setup["inputs"],
        &mut MockEngine::new(setup["guest_script"].clone()),
    )
    .unwrap();
    assert_eq!(live.records()[0]["version"], "0.1");
    let error = live.export_trace(&setup["inputs"]["trace_id"]).unwrap_err();
    assert!(
        error.contains("mixed") || error.contains("exact record bytes"),
        "{error}"
    );
}

#[test]
fn frozen_zero_one_archive_import_export_is_unchanged() {
    let setup = send();
    let mut live = Runtime::from_fixture(&setup).unwrap();
    live.step(
        &setup["inputs"],
        &mut MockEngine::new(setup["guest_script"].clone()),
    )
    .unwrap();
    let trace = live.export_trace(&setup["inputs"]["trace_id"]).unwrap();
    assert_eq!(trace["version"], "0.1");
    assert_eq!(
        Runtime::import_trace(&trace)
            .unwrap()
            .export_trace(&trace["trace_id"])
            .unwrap(),
        trace
    );
}

#[test]
fn daemon_manifest_admission_stays_closed_without_database_context() {
    let manifest = database_setup()["manifest"].clone();
    let error = admit_manifest(
        &manifest,
        &json!([]),
        &json!({"coglet":manifest["id"],"operations":["db.read","db.write"],"resources":[]}),
    )
    .unwrap_err();
    assert!(
        error.contains("authenticated Database/Catalog context"),
        "{error}"
    );
}
