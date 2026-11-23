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
        .seal_boundary(&send["inputs"]["trace_id"], 20, &[change])
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
