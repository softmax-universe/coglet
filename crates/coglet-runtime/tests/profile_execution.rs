//! Host/runtime profile proof, not qualification of live surface negotiation.
use coglet_runtime::{MockEngine, Runtime};
use serde_json::{Value, json};

#[test]
fn profile_router_and_committed_record_recover_without_inference() {
    use coglet_runtime::{ContractProfile, DurableStore, Router, RuntimeService};
    let mut setup = fixture();
    setup["contract_profile"] = json!("surface-0.2");
    setup["manifest"]["subscriptions"] = json!(["softmax.app.view_changed"]);
    let mut event = setup["inputs"]["events"][0].clone();
    event["kind"] = json!("softmax.app.view_changed");
    assert!(Router::new(8).unwrap().enqueue(event.clone()).is_err());
    let mut router = Router::with_contract_profile(8, ContractProfile::Surface02).unwrap();
    router.enqueue(event.clone()).unwrap();
    let decoded: Router = serde_json::from_value(serde_json::to_value(router).unwrap()).unwrap();
    assert_eq!(decoded.batch(1).unwrap(), vec![event.clone()]);
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("profile.wal");
    let mut service = RuntimeService::open(&wal, &setup, 8).unwrap();
    service.enqueue(event.clone()).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&wal, &setup, 8).unwrap();
    assert_eq!(service.queued(), 1);
    let result = service
        .run_next(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["events"], json!([event]));
    drop(service);
    let service = RuntimeService::open(&wal, &setup, 8).unwrap();
    assert_eq!(service.queued(), 0);
    assert_eq!(service.records(), &[result["journal"].clone()]);
    drop(service);
    // A valid transaction checksum is not authority to change one component.
    let mut store = DurableStore::open(&wal).unwrap();
    let mut state = store.state().unwrap().clone();
    state["router"]["contract_profile"] = json!("surface-0.1");
    store.commit(&state, None).unwrap();
    drop(store);
    assert!(RuntimeService::open(&wal, &setup, 8).is_err());
}

fn fixture() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone()
}

#[test]
fn inbox_and_empty_successor_retain_explicit_local_profile() {
    use coglet_runtime::ContractProfile;
    let mut setup = fixture();
    let mut event = setup["inputs"]["events"][0].clone();
    event["kind"] = json!("softmax.app.focus_changed");
    let message = json!({"index":0,"id":"5".repeat(32),"sender":event["source"],"event":event});
    let mut line = serde_json::to_vec(&message).unwrap();
    line.push(b'\n');
    setup["initial_files"]["/data/inbox.jsonl"] = json!(hex::encode(line));
    assert!(Runtime::from_fixture(&setup).is_err());
    setup["contract_profile"] = json!("surface-0.2");
    let mut runtime = Runtime::from_fixture(&setup).unwrap();
    let trace_id = json!("6".repeat(32));
    let change = json!({"actor":format!("hub:{}", "a".repeat(64)),
        "target":setup["manifest"]["id"],"audit_id":"8".repeat(32),"ticket":null,
        "method":"debug.write","path":"/data/note","before":null,"after":"61"});
    runtime.seal_boundary(&trace_id, 1, &[change]).unwrap();
    let next_id = json!("7".repeat(32));
    let next = runtime.start_segment(&next_id).unwrap();
    let trace = next.export_trace(&next_id).unwrap();
    assert!(Runtime::import_trace(&trace).is_err());
    let imported = Runtime::import_trace_with_profile(&trace, ContractProfile::Surface02).unwrap();
    assert_eq!(imported.export_trace(&next_id).unwrap(), trace);
    assert!(
        trace.get("contract_profile").is_none(),
        "no profile field added to frozen archives"
    );
}

#[test]
fn mapped_rules_are_explicit_and_unrelated_contracts_stay_frozen() {
    use coglet_runtime::ContractProfile::{Surface01, Surface02};
    for rule in [
        "common::event",
        "abi::call",
        "database::db-request",
        "surface::cap-entry",
    ] {
        assert_eq!(Surface01.rule(rule), rule);
        assert!(Surface02.rule(rule).starts_with("surface02::s02-"));
    }
    for rule in [
        "speech::start",
        "timer-request",
        "action-outcome",
        "database::db-reply",
        "unknown::event",
    ] {
        assert_eq!(Surface02.rule(rule), rule);
    }
    let mut event = fixture()["inputs"]["events"][0].clone();
    for invalid in [
        "softmax.apple.changed",
        "other.app.changed",
        "softmax.app.changed.more",
        "softmax.app.changed\n",
    ] {
        event["kind"] = json!(invalid);
        assert!(Surface02.validate("event", &event).is_err());
    }
}

#[test]
fn explicit_profile_reaches_manifest_event_import_and_step_record() {
    let mut setup = fixture();
    setup["manifest"]["subscriptions"] = json!(["softmax.app.view_changed"]);
    assert!(Runtime::from_fixture(&setup).is_err());
    setup["contract_profile"] = json!("surface-0.2");
    let mut runtime = Runtime::from_fixture(&setup).unwrap();
    let mut inputs = setup["inputs"].clone();
    inputs["events"][0]["kind"] = json!("softmax.app.view_changed");
    inputs["grants"] = json!(["softmax.app.navigate"]);
    let action = json!({"id":"1".repeat(32),"coglet":inputs["coglet"],
        "capability":"softmax.app.navigate","args":{},"deadline":1000000,
        "correlation_id":"2".repeat(32)});
    let mut script = setup["guest_script"].clone();
    script["calls"] = json!([{"op":"emit","argument":action,"result":0}]);
    let result = runtime.step(&inputs, &mut MockEngine::new(script)).unwrap();
    assert_eq!(result["journal"]["calls"][0]["result"], 0);
    assert_eq!(result["journal"]["actions"], json!([action]));
    coglet_runtime::validate("surface02::s02-step-record", &result["journal"]).unwrap();
    assert!(coglet_runtime::validate("step-record", &result["journal"]).is_err());
    let trace = runtime.export_trace(&inputs["trace_id"]).unwrap();
    assert!(Runtime::import_trace(&trace).is_err());
    let restored =
        Runtime::import_trace_with_profile(&trace, coglet_runtime::ContractProfile::Surface02)
            .unwrap();
    assert_eq!(
        restored.archive_state().unwrap(),
        runtime.archive_state().unwrap()
    );
    assert_eq!(restored.export_trace(&inputs["trace_id"]).unwrap(), trace);
    // Default execution still rejects app events before a guest attempt.
    let mut legacy = Runtime::from_fixture(&fixture()).unwrap();
    assert!(
        legacy
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .is_err()
    );
}
