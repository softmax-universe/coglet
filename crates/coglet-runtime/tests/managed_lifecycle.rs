use coglet_runtime::{DurableStore, Engine, EngineReport, MockEngine, RuntimeService, StepHost};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

fn fixture(name: &str) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["initial_files"]
        .as_object_mut()
        .unwrap()
        .retain(|p, _| !p.starts_with("/data/"));
    setup
}
struct Wal(PathBuf);
impl Wal {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("managed-lifecycle-{}-{name}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Self(root.join("state.wal"))
    }
}
impl Drop for Wal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}
fn identity() -> Value {
    json!({"initial":{"statement":{"new":"a".repeat(40)},"signature":"b".repeat(128)},
        "delegation":{"signature":"c".repeat(128)},"config":{"artifact":"d".repeat(64)}})
}
fn install(service: &mut RuntimeService) -> Value {
    service
        .managed_install(&identity(), &"1".repeat(32))
        .unwrap()
}
fn state(wal: &Wal) -> Value {
    DurableStore::open(&wal.0).unwrap().state().unwrap().clone()
}

#[test]
fn install_receipt_and_stopped_fence_survive_real_wal_reopen() {
    let setup = fixture("no-effects");
    let wal = Wal::new("install");
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    let original = install(&mut service);
    coglet_runtime::validate("hub-mutation-receipt", &original).unwrap();
    assert_eq!(service.managed_identity(), Some(&identity()));
    assert_eq!(service.managed_setup()["manifest"], setup["manifest"]);
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    let mut engine = MockEngine::new(setup["guest_script"].clone());
    assert!(service.run_next(&setup["inputs"], &mut engine).is_err());
    assert!(service.queue_due_timers(100).is_err());
    assert_eq!(service.queued(), 1);
    drop(service);
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert_eq!(service.managed_status()["state"], "stopped");
    assert_eq!(
        service
            .managed_install(&identity(), &"2".repeat(32))
            .unwrap(),
        original
    );
    let mut different = identity();
    different["config"]["artifact"] = json!("e".repeat(64));
    assert!(
        service
            .managed_install(&different, &"2".repeat(32))
            .unwrap_err()
            .starts_with("conflict:")
    );
    service.managed_start().unwrap();
    service.managed_start().unwrap();
    service
        .run_next(&setup["inputs"], &mut engine)
        .unwrap()
        .unwrap();
    assert_eq!(service.records().len(), 1);
    drop(service);
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert_eq!(service.managed_status()["state"], "running");
    service.managed_stop(100).unwrap();
    let status = service.managed_status();
    coglet_runtime::validate("hub-status", &status).unwrap();
    assert_eq!(
        status,
        json!({"state":"stopped","live_commit":"a".repeat(40),"step":0,"pending":0,"error":null})
    );
    drop(service);
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert_eq!(service.managed_status(), status);
    assert!(service.run_next(&setup["inputs"], &mut engine).is_err());
    assert_eq!(service.records().len(), 1);
}

#[test]
fn install_rejects_private_initial_data_but_accepts_absent_or_empty_inbox() {
    for (name, extra, valid) in [
        ("empty", None, true),
        ("inbox", Some(("/data/inbox.jsonl", "")), true),
        ("private", Some(("/data/private", "")), false),
        ("private-content", Some(("/data/private", "aa")), false),
    ] {
        let mut setup = fixture("no-effects");
        // Some fixtures initialize /data/count: remove all private provisioning.
        setup["initial_files"]
            .as_object_mut()
            .unwrap()
            .retain(|p, _| !p.starts_with("/data/"));
        if let Some((path, bytes)) = extra {
            setup["initial_files"][path] = json!(bytes);
        }
        let wal = Wal::new(name);
        let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
        assert_eq!(
            service
                .managed_install(&identity(), &"1".repeat(32))
                .is_ok(),
            valid,
            "{name}"
        );
    }
}

#[test]
fn stop_cancels_undelivered_actions_without_erasing_abi_history() {
    let setup = fixture("action");
    let wal = Wal::new("cancel");
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    install(&mut service);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    let record = service
        .run_next(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone()),
        )
        .unwrap()
        .unwrap()["journal"]
        .clone();
    service.managed_stop(100).unwrap();
    assert_eq!(service.records(), std::slice::from_ref(&record));
    service.managed_stop(100).unwrap();
    drop(service);
    let saved = state(&wal);
    let staged = &saved["staged_boundary"];
    assert!(
        staged["runtime"]["pending_actions"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert_eq!(staged["changes"].as_array().unwrap().len(), 1);
    let bytes = hex::decode(
        staged["runtime"]["files"]["/data/inbox.jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let terminal: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(terminal["event"]["payload"]["error"]["code"], "cancelled");
    assert_eq!(
        saved["runtime"]["ledger"]["actions"],
        json!([record["actions"][0]["id"]])
    );
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert_eq!(service.records(), &[record]);
    assert_eq!(service.managed_status()["state"], "stopped");
    service.managed_start().unwrap();
    let mut context = setup["inputs"].clone();
    context["logical_time"] = json!(100);
    let result = service
        .run_next(
            &context,
            &mut MockEngine::new(fixture("no-effects")["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["boundary"].as_array().unwrap().len(), 1);
}

struct Unknown;
impl Engine for Unknown {
    fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
        Err("worker observation lost".into())
    }
}
#[test]
fn stop_preserves_unknown_attempt_fence_and_exact_reserved_inputs_across_reopen() {
    let setup = fixture("no-effects");
    let wal = Wal::new("unknown");
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    install(&mut service);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    assert!(service.run_next(&setup["inputs"], &mut Unknown).is_err());
    service.managed_stop(100).unwrap();
    assert!(service.managed_start().is_err());
    coglet_runtime::validate("hub-status", &service.managed_status()).unwrap();
    drop(service);
    let before = state(&wal);
    assert!(before["runtime"]["unresolved_attempt"].is_object());
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert_eq!(service.managed_status()["state"], "stopped");
    assert!(service.managed_start().is_err());
    service.managed_stop(200).unwrap();
    drop(service);
    let after = state(&wal);
    assert_eq!(
        before["runtime"]["unresolved_attempt"],
        after["runtime"]["unresolved_attempt"]
    );
    assert_eq!(before["router"], after["router"]);
}

#[test]
fn stopped_unknown_attempt_reconciliation_cannot_revive_undelivered_actions() {
    let setup = fixture("action");
    let wal = Wal::new("unknown-reconciled");
    let mut runtime = coglet_runtime::Runtime::from_fixture(&setup).unwrap();
    let original_observation = runtime
        .step(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone()),
        )
        .unwrap()["journal"]
        .clone();
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    install(&mut service);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    assert!(service.run_next(&setup["inputs"], &mut Unknown).is_err());
    service.managed_stop(300).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    service.recover_attempt(&original_observation).unwrap();
    assert_eq!(service.managed_status()["state"], "stopped");
    assert_eq!(service.records(), &[original_observation]);
    service.managed_start().unwrap();
    drop(service);
    let saved = state(&wal);
    assert!(
        saved["staged_boundary"]["runtime"]["pending_actions"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    let change = &saved["staged_boundary"]["changes"][0];
    let terminal: Value =
        serde_json::from_slice(&hex::decode(change["after"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(terminal["event"]["payload"]["error"]["code"], "cancelled");
    assert_eq!(terminal["event"]["logical_time"], 300);
}

#[test]
fn full_router_cancellation_failure_still_durably_fences_execution() {
    let setup = fixture("action");
    let wal = Wal::new("full-stop");
    let mut service = RuntimeService::open(&wal.0, &setup, 1).unwrap();
    install(&mut service);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    service
        .run_next(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    let mut event = setup["inputs"]["events"][0].clone();
    event["seq"] = json!(event["seq"].as_u64().unwrap() + 1);
    service.enqueue(event).unwrap();
    assert!(service.managed_stop(100).is_err());
    assert_eq!(service.managed_status()["state"], "stopped");
    drop(service);
    let mut service = RuntimeService::open(&wal.0, &setup, 1).unwrap();
    assert_eq!(service.managed_status()["state"], "stopped");
    assert!(service.managed_start().is_err());
    assert!(service.run_next(&setup["inputs"], &mut Unknown).is_err());
    assert_eq!(service.records().len(), 1);
}

#[test]
fn legacy_unmanaged_execution_is_unchanged() {
    let setup = fixture("no-effects");
    let wal = Wal::new("legacy");
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert!(service.managed_identity().is_none());
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    assert!(
        service
            .run_next(
                &setup["inputs"],
                &mut MockEngine::new(setup["guest_script"].clone())
            )
            .unwrap()
            .is_some()
    );
    assert!(
        service
            .managed_install(&identity(), &"1".repeat(32))
            .is_err()
    );
}

#[test]
fn stopped_dispatch_refuses_fresh_effects_but_reconciles_original_envelope_and_outcome() {
    use coglet_runtime::{SurfaceBinding, SurfaceSession, cbor};
    let coglet = format!("coglet:{}", "a".repeat(64));
    let adapter = format!("surface:{}", "b".repeat(64));
    let action = json!({"coglet":coglet,"id":"4".repeat(32),"capability":"expression.set_expression",
        "args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":20_000,"correlation_id":"5".repeat(32)});
    let mut later = action.clone();
    later["id"] = json!("6".repeat(32));
    let mut setup = fixture("no-effects");
    setup["manifest"]["id"] = json!(coglet);
    setup["initial_actions"] = json!([action, later]);
    setup["initial_effect_ids"]["actions"] = json!([action["id"], later["id"]]);
    let wal = Wal::new("dispatch");
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    install(&mut service);
    let mut session =
        SurfaceSession::new(SurfaceBinding::new(adapter.clone(), vec![coglet.clone()]).unwrap());
    for (kind, body, now) in [
        (
            "hello",
            json!({"surface":adapter,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
            10,
        ),
        (
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[{"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}),
            110,
        ),
        (
            "grants",
            json!({"epoch":1,"entries":[{"coglet":coglet,"operations":["expression.set_expression"],"resources":["display.face"],"expires":1_000_000}],"correlation_id":"3".repeat(32)}),
            120,
        ),
    ] {
        assert_eq!(
            session.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), now),
            0
        );
    }
    let id = action["id"].as_str().unwrap();
    assert!(service.prepare_dispatch(&session, id, 130).is_err());
    service.managed_start().unwrap();
    let bytes = service.prepare_dispatch(&session, id, 130).unwrap();
    assert!(!bytes.is_empty());
    service.managed_stop(140).unwrap();
    assert!(service.prepare_dispatch(&session, id, 150).is_err());
    assert!(
        service
            .prepare_dispatch(&session, later["id"].as_str().unwrap(), 150)
            .is_err()
    );
    drop(service);
    let mut service = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert!(service.prepare_dispatch(&session, id, 160).is_err());
    service
        .record_surface_accept(
            &session,
            &json!({"id":id,"correlation_id":action["correlation_id"],"accepted":true}),
            170,
        )
        .unwrap();
    service.record_surface_outcome(&session, &json!({"source":adapter,"kind":"action.outcome","seq":1,"logical_time":180,"correlation_id":action["correlation_id"],"payload":{"id":id,"correlation_id":action["correlation_id"],"status":"succeeded","result":{}}}), 180).unwrap();
    assert_eq!(service.managed_status()["state"], "stopped");
    drop(service);
    let saved = state(&wal);
    assert_eq!(saved["dispatches"][id]["outcome"]["status"], "succeeded");
    assert!(
        saved["staged_boundary"]["runtime"]["pending_actions"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[cfg(feature = "native-engine")]
#[test]
fn native_scheduler_obeys_durable_stop_and_resumes_only_after_explicit_start() {
    use coglet_runtime::{NativeSchedule, SurfaceBinding, SurfaceSession, cbor, digest};
    use std::sync::{Arc, Mutex};
    let mut setup = fixture("no-effects");
    let wasm = wat::parse_str("(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))").unwrap();
    setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&wasm));
    let raw = hex::decode(
        setup["initial_files"]["/code/build/provenance.cbor"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut provenance: Value = coglet_types::decode("hub-build-record", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    provenance["rewritten_sha256"] = json!(digest(&wasm));
    setup["initial_files"]["/code/build/provenance.cbor"] =
        json!(hex::encode(cbor(&provenance).unwrap()));
    setup["manifest"]["budgets"]["compute_mode"] = json!(coglet_engine_wamr::COMPUTE_MODE);
    setup["manifest"]["budgets"]["memory_pages"] = json!(2);
    setup["manifest"]["budgets"]["wall_time_us"] = json!(2_000_000);
    setup["manifest"]["subscriptions"] = json!([]);
    setup["manifest"]["required_capabilities"] = json!([]);
    setup["manifest"]["exclusive_outputs"] = json!([]);
    let wal = Wal::new("native-scheduler");
    let mut runtime = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    install(&mut runtime);
    runtime
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    runtime
        .enable_native(NativeSchedule {
            trace_id: setup["inputs"]["trace_id"].as_str().unwrap().into(),
            seed: 1,
            runtime_bytes: 8_388_608,
            stack_bytes: 65_536,
        })
        .unwrap();
    let service = Arc::new(Mutex::new(runtime));
    let coglet = setup["manifest"]["id"].as_str().unwrap();
    let adapter = format!("surface:{}", "b".repeat(64));
    let mut session = SurfaceSession::with_service(
        SurfaceBinding::new(adapter.clone(), vec![coglet.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    for (kind, body, now) in [
        (
            "hello",
            json!({"surface":adapter,"offer":{"versions":["0.1"]},"clock":10,"correlation_id":"1".repeat(32)}),
            10,
        ),
        (
            "capabilities",
            json!({"revision":1,"clock":110,"entries":[],"correlation_id":"2".repeat(32)}),
            110,
        ),
        (
            "grants",
            json!({"epoch":1,"entries":[{"coglet":coglet,"operations":[],"resources":[],"expires":10_000_000}],"correlation_id":"3".repeat(32)}),
            120,
        ),
    ] {
        assert_eq!(
            session.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), now),
            0
        );
    }
    session.pump_dispatch(130).unwrap();
    assert_eq!(service.lock().unwrap().records().len(), 0);
    service.lock().unwrap().managed_start().unwrap();
    session.pump_dispatch(140).unwrap();
    assert_eq!(service.lock().unwrap().records().len(), 1);
    assert_eq!(service.lock().unwrap().records()[0]["status"], "committed");
    service.lock().unwrap().managed_stop(150).unwrap();
    session.pump_dispatch(160).unwrap();
    assert_eq!(service.lock().unwrap().records().len(), 1);
    drop(session);
    drop(service);
    let mut reopened = RuntimeService::open(&wal.0, &setup, 8).unwrap();
    assert_eq!(reopened.managed_status()["state"], "stopped");
    assert_eq!(reopened.records().len(), 1);
    assert!(reopened.queue_due_timers(170).is_err());
}
