use super::*;
#[test]
fn native_dispatch_rechecks_clock_after_guest_outlives_grant() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut setup = with_wasm(setup(), &program_delay(false, true));
    setup["manifest"]["budgets"]["compute"] = json!(1_000_000_000);
    setup["manifest"]["budgets"]["wall_time_us"] = json!(10_000_000);
    let dir = std::env::temp_dir().join(format!("native-expiry-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut service = RuntimeService::open(&dir.join("wal"), &setup, 128).unwrap();
    service
        .enable_native(serde_json::from_value(setup["native_schedule"].clone()).unwrap())
        .unwrap();
    let start = service.supervised_now().unwrap();
    let service = std::sync::Arc::new(Mutex::new(service));
    let mut session = library_session(service.clone());
    // Replace the generous epoch with a lease valid now, but shorter than the
    // intentionally expensive real Wasm initializer. Refresh the mapping too.
    let mut admitted = None;
    // A loaded worker can spend the entire short lease in setup fsyncs. Retry
    // only fixture establishment, before ANY guest attempt, under a fresh epoch
    // and clock sample. Keep the real expiry and no-dispatch assertions intact.
    for revision in 2..18 {
        let now = service.lock().unwrap().supervised_now().unwrap();
        let caps = json!({"type":"capabilities","body":{"revision":revision,"clock":now+1000,
        "entries":[{"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}});
        assert_eq!(session.send(&cbor(&caps).unwrap(), now), 0);
        let grant = json!({"type":"grants","body":{"epoch":revision,"entries":[{"coglet":C,"operations":["expression.set_expression"],"resources":["display.face"],"expires":now+1000+250_000}],"correlation_id":"3".repeat(32)}});
        assert_eq!(session.send(&cbor(&grant).unwrap(), now), 0);
        let before = service.lock().unwrap().supervised_now().unwrap();
        if before < now + 250_000 {
            admitted = Some((now, before));
            break;
        }
    }
    let (now, before) = admitted
        .expect("could not establish an unexpired short fixture lease before guest admission");
    session.pump_dispatch(before).unwrap();
    let after = service.lock().unwrap().supervised_now().unwrap();
    assert!(after > now + 250_000, "initializer did not outlive lease");
    assert!(after > start);
    assert!(session.pop().is_none(), "expired lease exposed a fresh act");
    let guard = service.lock().unwrap();
    assert_eq!(guard.records().len(), 1);
    assert_eq!(guard.records()[0]["status"], "committed");
    assert_eq!(guard.records()[0]["actions"].as_array().unwrap().len(), 2);
    drop(guard);
    drop(session);
    drop(service);
    let store = coglet_runtime::DurableStore::open(&dir.join("wal")).unwrap();
    assert!(
        store.state().unwrap()["dispatches"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
