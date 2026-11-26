//! ABI 0.2 archive export/import of exact lossless DB records, and the daemon
//! admission boundary. Offline archive validation is not guest replay.
use coglet_runtime::{MockEngine, Runtime, admit_manifest, cbor, digest};
use coglet_types_current as coglet_types;
use serde_json::{Value, json};

const CATALOG: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn send() -> Value {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup =
        cases.iter().find(|c| c["name"] == "no-effects").unwrap()["steps"][0]["send"].clone();
    setup["manifest"]["version"] = json!("1.0");
    setup
}

fn database_setup() -> Value {
    let mut setup = send();
    let mut manifest = setup["manifest"].clone();
    manifest["version"] = json!("1.1");
    manifest["database"] = json!({"schema_min":1,"schema_max":1,"catalog_hash":CATALOG});
    setup["manifest"] = manifest;
    // Trusted fixture observation, not an inferred fresh-database revision.
    setup["initial_db_revision"] = json!(3);
    setup
}

fn db_call() -> Value {
    json!({"op":"db","argument":{"operation":"statement","statement":"scan","mode":"read",
        "params":[],"max_rows":64,"max_bytes":65536},"result":0,
        "response":{"status":"page","columns":["id"],"rows":[],"affected":0,"revision":3,"next_cursor":null}})
}

fn exact_record(mut record: Value) -> (Value, Vec<u8>) {
    record["version"] = json!("1.1");
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
    assert_eq!(trace["version"], "1.1");
    assert!(trace["records"].as_array().unwrap().is_empty());
    assert_eq!(trace["initial_db_revision"], 3);
    assert_eq!(trace["final_db_revision"], 3);
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
    assert_eq!(exported["version"], "1.1");
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
    assert_eq!(live.records()[0]["version"], "1.0");
    let error = live.export_trace(&setup["inputs"]["trace_id"]).unwrap_err();
    assert!(
        error.contains("mixed") || error.contains("exact record bytes"),
        "{error}"
    );
}

#[test]
fn current_plain_archive_import_export_roundtrips() {
    let setup = send();
    let mut live = Runtime::from_fixture(&setup).unwrap();
    live.step(
        &setup["inputs"],
        &mut MockEngine::new(setup["guest_script"].clone()),
    )
    .unwrap();
    let trace = live.export_trace(&setup["inputs"]["trace_id"]).unwrap();
    assert_eq!(trace["version"], "1.0");
    assert_eq!(
        Runtime::import_trace(&trace)
            .unwrap()
            .export_trace(&trace["trace_id"])
            .unwrap(),
        trace
    );
}

#[test]
fn unknown_database_revision_never_exports_a_fabricated_zero() {
    let mut setup = database_setup();
    setup.as_object_mut().unwrap().remove("initial_db_revision");
    let runtime = Runtime::from_fixture(&setup).unwrap();
    assert!(
        runtime
            .export_trace(&setup["inputs"]["trace_id"])
            .unwrap_err()
            .contains("revision")
    );
}

#[test]
fn database_boundary_seal_and_empty_successor_keep_observed_revision_across_wal_reopen() {
    let setup = database_setup();
    let path = std::env::temp_dir().join(format!("coglet-db-boundary-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 64).unwrap();
    let id = &setup["inputs"]["trace_id"];
    let change = json!({"actor":format!("hub:{}","3".repeat(64)),"target":setup["manifest"]["id"],
        "audit_id":"d".repeat(32),"ticket":null,"method":"debug.write","path":"/data/count","before":"30","after":"31"});
    let record = service.seal_boundary(id, 20, &[change]).unwrap();
    assert_eq!(record["version"], "1.1");
    drop(service);
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 64).unwrap();
    let trace = service.export_trace(id, 20).unwrap();
    assert_eq!(trace["trailing_boundary"], record);
    let imported = Runtime::import_trace(&trace).unwrap();
    assert_eq!(imported.export_trace(id).unwrap(), trace);
    let mut corrupt = trace.clone();
    corrupt["trailing_boundary"]["logical_time"] = json!(21);
    assert!(Runtime::import_trace(&corrupt).is_err());
    let next_id = json!("e".repeat(32));
    service.start_segment(&next_id).unwrap();
    drop(service);
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 64).unwrap();
    assert_eq!(service.export_segment(id).unwrap(), trace);
    let empty = service.export_trace(&next_id, 20).unwrap();
    assert_eq!(empty["initial_db_revision"], 3);
    assert_eq!(empty["final_db_revision"], 3);
    assert!(empty["records"].as_array().unwrap().is_empty());
    assert_eq!(
        Runtime::import_trace(&empty)
            .unwrap()
            .export_trace(&next_id)
            .unwrap(),
        empty
    );
    drop(service);
    std::fs::remove_file(path).unwrap();
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
