use coglet_runtime::Runtime;
use serde_json::Value;

fn corpus(text: &str) -> Vec<Value> {
    serde_json::from_str(text).unwrap()
}

#[test]
fn complete_frozen_archive_profiles_validate_without_guest_execution() {
    for text in [
        include_str!("../../../conformance/abi/boundary-receipts.json"),
        include_str!("../../../conformance/abi/provenance.json"),
        include_str!("../../../conformance/abi/heard-archive.json"),
    ] {
        for case in corpus(text) {
            let steps = case["steps"].as_array().unwrap();
            let trace = &steps.iter().find_map(|s| s.get("send")).unwrap()["trace"];
            let expected = steps
                .iter()
                .find_map(|s| s.get("expect").or_else(|| s.get("match")))
                .unwrap();
            let imported = Runtime::import_trace(trace);
            assert_eq!(
                imported.is_ok(),
                expected["valid"].as_bool().unwrap(),
                "{}: {:?}",
                case["name"],
                imported.as_ref().err()
            );
            if let Ok(runtime) = imported {
                assert_eq!(
                    runtime.export_trace(&trace["trace_id"]).unwrap(),
                    *trace,
                    "roundtrip {}",
                    case["name"]
                );
                if !expected["state"].is_null() {
                    assert_eq!(
                        runtime.archive_state().unwrap(),
                        expected["state"],
                        "state {}",
                        case["name"]
                    );
                }
            }
        }
    }
}

#[test]
fn imported_archives_retain_unreferenced_verified_blobs() {
    use coglet_runtime::digest;
    use serde_json::json;
    let cases = corpus(include_str!("../../../conformance/abi/provenance.json"));
    let mut trace = cases
        .iter()
        .map(|c| &c["steps"][0]["send"]["trace"])
        .find(|t| Runtime::import_trace(t).is_ok())
        .unwrap()
        .clone();
    let bytes = b"retained archive attachment";
    trace["blobs"][digest(bytes)] = json!(hex::encode(bytes));
    let imported = Runtime::import_trace(&trace).unwrap();
    assert_eq!(imported.export_trace(&trace["trace_id"]).unwrap(), trace);
}

#[test]
fn durable_service_export_includes_reopened_journal() {
    use coglet_runtime::{MockEngine, RuntimeService};
    let cases = corpus(include_str!("../../../conformance/abi/cases.json"));
    let send = &cases.iter().find(|c| c["name"] == "timer").unwrap()["steps"][0]["send"];
    let directory = std::env::temp_dir().join(format!("coglet-export-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut service = RuntimeService::open(&path, send, 8).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    service
        .run_next(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    drop(service);
    let mut service = RuntimeService::open(&path, send, 8).unwrap();
    let trace = service
        .export_trace(&send["inputs"]["trace_id"], 10)
        .unwrap();
    assert_eq!(trace["records"].as_array().unwrap().len(), 1);
    assert_eq!(
        Runtime::import_trace(&trace)
            .unwrap()
            .export_trace(&trace["trace_id"])
            .unwrap(),
        trace
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn durable_segment_transfer_retains_history_router_and_trace_local_chain() {
    use coglet_runtime::{MockEngine, RuntimeService};
    use serde_json::json;
    let cases = corpus(include_str!("../../../conformance/abi/cases.json"));
    let send = &cases.iter().find(|c| c["name"] == "timer").unwrap()["steps"][0]["send"];
    let directory = std::env::temp_dir().join(format!("coglet-segment-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut service = RuntimeService::open(&path, send, 8).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    service
        .run_next(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let change = json!({"actor":format!("hub:{}","3".repeat(64)),"target":send["manifest"]["id"],"audit_id":"d".repeat(32),
        "ticket":null,"method":"debug.write","path":"/data/count","before":"30","after":"31"});
    service
        .seal_boundary(
            &send["inputs"]["trace_id"],
            20,
            std::slice::from_ref(&change),
        )
        .unwrap();
    let old = service
        .export_trace(&send["inputs"]["trace_id"], 20)
        .unwrap();
    let now = send["guest_script"]["calls"][0]["argument"]["at"]
        .as_u64()
        .unwrap()
        .max(20);
    service.queue_due_timers(now).unwrap();
    let queued = service.queued();
    assert!(queued > 0);
    let new_id = json!("e".repeat(32));
    service.start_segment(&new_id).unwrap();
    assert!(service.start_segment(&new_id).is_err());
    assert_eq!(service.queued(), queued);
    assert_eq!(service.export_segment(&old["trace_id"]).unwrap(), old);
    drop(service);
    let mut service = RuntimeService::open(&path, send, 8).unwrap();
    let fresh = service.export_trace(&new_id, now).unwrap();
    assert_eq!(fresh["initial_hash"], old["final_hash"]);
    assert_eq!(
        fresh["initial_effect_ids"],
        json!({"actions":[],"timers":old["records"][0]["timers"].as_array().unwrap().iter().map(|t|t["id"].clone()).collect::<Vec<_>>(),"llm_requests":[]})
    );
    assert!(fresh["records"].as_array().unwrap().is_empty());
    Runtime::import_trace(&fresh).unwrap();
    drop(service);
    let mut store = coglet_runtime::DurableStore::open(&path).unwrap();
    let good = store.state().unwrap().clone();
    let mut bad = good.clone();
    bad["runtime"]["time"] = json!(0);
    store.commit(&bad, None).unwrap();
    drop(store);
    assert!(
        RuntimeService::open(&path, send, 8).is_err(),
        "empty successor lost predecessor clock floor"
    );
    let mut store = coglet_runtime::DurableStore::open(&path).unwrap();
    store.commit(&good, None).unwrap();
    drop(store);
    let mut service = RuntimeService::open(&path, send, 8).unwrap();
    let no_effects = cases.iter().find(|c| c["name"] == "no-effects").unwrap()["steps"][0]["send"]
        ["guest_script"]
        .clone();
    let mut inputs = send["inputs"].clone();
    inputs["trace_id"] = new_id.clone();
    inputs["logical_time"] = json!(now);
    let result = service
        .run_next(&inputs, &mut MockEngine::new(no_effects))
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["step"], 0);
    assert!(result["journal"]["previous"].is_null());
    assert_eq!(service.export_segment(&old["trace_id"]).unwrap(), old);
    drop(service);
    let mut service = RuntimeService::open(&path, send, 8).unwrap();
    let trace = service.export_trace(&new_id, now).unwrap();
    assert_eq!(trace["records"].as_array().unwrap().len(), 1);
    assert_eq!(service.records().len(), 3);
    Runtime::import_trace(&trace).unwrap();
    // A third empty segment has no timer/event to accidentally enforce the
    // predecessor's clock/sequence floor for it.
    let mut change2 = change.clone();
    change2["before"] = json!("31");
    change2["after"] = json!("32");
    change2["audit_id"] = json!("9".repeat(32));
    service.seal_boundary(&new_id, now, &[change2]).unwrap();
    let third_id = json!("f".repeat(32));
    service.start_segment(&third_id).unwrap();
    drop(service);
    for field in ["time", "next_source_seq"] {
        let mut store = coglet_runtime::DurableStore::open(&path).unwrap();
        let good = store.state().unwrap().clone();
        let mut bad = good.clone();
        bad["runtime"][field] = json!(0);
        store.commit(&bad, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, send, 8).is_err(),
            "third segment lost {field} floor"
        );
        let mut store = coglet_runtime::DurableStore::open(&path).unwrap();
        store.commit(&good, None).unwrap();
    }
    // Even a checksum-valid internal transaction cannot substitute a segment
    // root, invent a cutoff, or change the transferred lifetime ledger.
    for mutation in ["root", "cutoff", "active-ledger", "reused-id"] {
        let mut store = coglet_runtime::DurableStore::open(&path).unwrap();
        let good = store.state().unwrap().clone();
        let mut bad = good.clone();
        match mutation {
            "root" => {
                bad["completed_segments"][0]["initial_setup"]["initial_files"]["/data/count"] =
                    json!("39")
            }
            "cutoff" => bad["completed_segments"][0]["end"] = json!(1),
            "active-ledger" => {
                bad["runtime"]["initial_setup"]["initial_effect_ids"]["actions"] =
                    json!(["f".repeat(32)])
            }
            _ => bad["runtime"]["trace_id"] = old["trace_id"].clone(),
        }
        store.commit(&bad, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, send, 8).is_err(),
            "forged {mutation} accepted"
        );
        let mut store = coglet_runtime::DurableStore::open(&path).unwrap();
        store.commit(&good, None).unwrap();
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn every_primary_script_reexecutes_independently_with_identical_step_hashes() {
    use coglet_runtime::{Engine, EngineReport, MockEngine, StepHost};
    struct Programs(Vec<Value>);
    impl Engine for Programs {
        fn execute(
            &mut self,
            inputs: &Value,
            host: &mut StepHost,
        ) -> coglet_runtime::Result<EngineReport> {
            assert!(inputs.get("output_files").is_none());
            assert!(inputs.get("calls").is_none());
            assert!(inputs.get("usage").is_none());
            let step = inputs["step"].as_u64().unwrap() as usize;
            MockEngine::new(self.0[step].clone()).execute(inputs, host)
        }
    }
    let cases = corpus(include_str!("../../../conformance/abi/cases.json"));
    for case in cases {
        let requests: Vec<_> = case["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|step| step.get("send"))
            .collect();
        let mut live = Runtime::from_fixture(requests[0]).unwrap();
        let mut hashes = Vec::new();
        let programs: Vec<_> = requests
            .iter()
            .map(|request| request["guest_script"].clone())
            .collect();
        for request in &requests {
            let result = live
                .step(
                    &request["inputs"],
                    &mut MockEngine::new(request["guest_script"].clone()),
                )
                .unwrap();
            hashes.push(result["state_hash"].as_str().unwrap().to_owned());
        }
        let replayed = Runtime::from_fixture(requests[0])
            .unwrap()
            .replay(live.records(), &mut Programs(programs))
            .unwrap();
        assert_eq!(hashes, replayed, "{}", case["name"]);
    }
}

#[test]
fn independent_program_replay_and_segment_transfer_preserve_lifetime_state() {
    use coglet_runtime::MockEngine;
    use serde_json::json;
    let cases = corpus(include_str!("../../../conformance/abi/cases.json"));
    let case = cases.iter().find(|c| c["name"] == "timer").unwrap();
    let send = &case["steps"][0]["send"];
    let mut runtime = Runtime::from_fixture(send).unwrap();
    let result = runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let trace = runtime.export_trace(&send["inputs"]["trace_id"]).unwrap();
    assert_eq!(
        Runtime::import_trace(&trace)
            .unwrap()
            .archive_state()
            .unwrap(),
        runtime.archive_state().unwrap()
    );
    let hashes = Runtime::from_fixture(send)
        .unwrap()
        .replay(
            runtime.records(),
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    assert_eq!(hashes, vec![result["state_hash"].as_str().unwrap()]);
    let change = json!({"actor":format!("hub:{}","3".repeat(64)),"target":send["manifest"]["id"],"audit_id":"d".repeat(32),
        "ticket":null,"method":"debug.write","path":"/data/count","before":"30","after":"31"});
    runtime
        .seal_boundary(
            &send["inputs"]["trace_id"],
            20,
            std::slice::from_ref(&change),
        )
        .unwrap();
    let new_id = json!("e".repeat(32));
    let segment = runtime.start_segment(&new_id).unwrap();
    let exported = segment.export_trace(&new_id).unwrap();
    assert!(exported["records"].as_array().unwrap().is_empty());
    assert_eq!(exported["initial_files"]["/data/count"], "31");
    assert_eq!(
        exported["initial_timers"],
        json!([send["guest_script"]["calls"][0]["argument"]])
    );
    assert_eq!(
        segment.archive_state().unwrap(),
        runtime.archive_state().unwrap()
    );
    Runtime::import_trace(&exported).unwrap();
}
