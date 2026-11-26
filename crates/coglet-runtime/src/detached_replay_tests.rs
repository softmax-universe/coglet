//! Algorithmic regression: live reconciliation must not replay its entire prefix.
use super::*;

fn fixture() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone()
}

#[test]
fn runtime_clone_shares_immutable_history_and_append_is_isolated() {
    let setup = fixture();
    let mut original = Runtime::from_fixture(&setup).unwrap();
    let mut input = setup["inputs"].clone();
    for step in 0..2 {
        input["step"] = json!(step);
        input["previous"] = original
            .records()
            .last()
            .map(|r| r["record_hash"].clone())
            .unwrap_or(Value::Null);
        original
            .step(&input, &mut MockEngine::new(setup["guest_script"].clone()))
            .unwrap();
    }
    let saved = original.records().to_vec();
    let serialized = serde_json::to_value(&original).unwrap();
    let mut cloned = original.clone();
    assert!(
        std::ptr::eq(original.records().as_ptr(), cloned.records().as_ptr()),
        "cloning live state must not copy immutable historical record bytes"
    );
    assert_eq!(serde_json::to_value(&cloned).unwrap(), serialized);
    assert!(serialized.get("records").is_none());
    input["step"] = json!(2);
    input["previous"] = original.records().last().unwrap()["record_hash"].clone();
    cloned
        .step(&input, &mut MockEngine::new(setup["guest_script"].clone()))
        .unwrap();
    assert_eq!(original.records(), saved.as_slice());
    assert_eq!(cloned.records().len(), 3);
    assert_eq!(&cloned.records()[..2], saved.as_slice());
    assert!(!std::ptr::eq(
        original.records().as_ptr(),
        cloned.records().as_ptr()
    ));
}

#[test]
fn live_detached_replays_only_new_observed_transition() {
    for prefix in [2, 8] {
        let dir = tempfile::tempdir().unwrap();
        let setup = fixture();
        let mut service = RuntimeService::open(&dir.path().join("state.wal"), &setup, 32).unwrap();
        for step in 0..prefix {
            let mut event = setup["inputs"]["events"][0].clone();
            event["seq"] = json!(step);
            service.enqueue(event).unwrap();
            service
                .run_next(
                    &setup["inputs"],
                    &mut MockEngine::new(setup["guest_script"].clone()),
                )
                .unwrap()
                .unwrap();
        }
        let mut event = setup["inputs"]["events"][0].clone();
        event["seq"] = json!(prefix);
        service.enqueue(event).unwrap();
        let run = service.prepare_detached(&setup["inputs"]).unwrap().unwrap();
        let shared = Arc::new(Mutex::new(service));
        crate::execution::trace::OBSERVED_STEPS.with(|count| count.set(0));
        RuntimeService::execute_detached(
            &shared,
            run,
            &mut MockEngine::new(setup["guest_script"].clone()),
            &|f| f(),
            &|_, _| Ok(()),
        )
        .unwrap();
        let observed = crate::execution::trace::OBSERVED_STEPS.with(|count| count.get());
        assert_eq!(
            observed, 1,
            "prefix length {prefix} must not be replayed on the live path"
        );
        let locked = shared.lock().unwrap();
        assert_eq!(locked.records().len(), prefix + 1);
        assert_eq!(locked.queued(), 0);
        assert!(locked.state.runtime.unresolved_attempt.is_none());
    }
}
