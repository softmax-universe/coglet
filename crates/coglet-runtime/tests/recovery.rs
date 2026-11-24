use coglet_runtime::{DurableStore, MockEngine, Runtime, RuntimeService};
use serde_json::{Value, json};
use std::fs;

fn setup() -> Value {
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
fn ambiguous_engine_error_is_fenced_in_memory_and_after_reopen() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct Unavailable(usize);
    impl Engine for Unavailable {
        fn execute(
            &mut self,
            _: &Value,
            host: &mut StepHost,
        ) -> coglet_runtime::Result<EngineReport> {
            self.0 += 1;
            host.guest_write("/data/count", b"uncertain")?;
            Err("engine transport lost after entry".into())
        }
    }
    let send = setup();
    let directory = std::env::temp_dir().join(format!("coglet-attempt-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut engine = Unavailable(0);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    assert!(service.run_next(&send["inputs"], &mut engine).is_err());
    assert!(service.run_next(&send["inputs"], &mut engine).is_err());
    assert_eq!(engine.0, 1, "ambiguous attempt automatically reran");
    assert_eq!(service.queued(), 1);
    assert!(
        service.records().is_empty(),
        "unknown usage fabricated a failure record"
    );
    drop(service);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    assert!(service.run_next(&send["inputs"], &mut engine).is_err());
    assert_eq!(engine.0, 1, "reopen silently cleared the attempt fence");
    assert!(
        service
            .export_trace(&send["inputs"]["trace_id"], 10)
            .is_err(),
        "unknown attempt exported as complete"
    );
    // An authenticated worker can recover the actual completed observation,
    // never request a fresh execution of the retained input.
    let mut original = Runtime::from_fixture(&send).unwrap();
    let observed = original
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let mut wrong = observed["journal"].clone();
    wrong["seed"] = json!(999);
    assert!(service.recover_attempt(&wrong).is_err());
    service.recover_attempt(&observed["journal"]).unwrap();
    assert_eq!(service.queued(), 0);
    assert_eq!(service.records(), &[observed["journal"].clone()]);
    assert!(service.recover_attempt(&observed["journal"]).is_err());
    drop(service);
    let service = RuntimeService::open(&path, &send, 4).unwrap();
    assert_eq!(service.queued(), 0);
    drop(service);
    let store = DurableStore::open(&path).unwrap();
    assert_eq!(
        store.state().unwrap()["runtime"]["files"],
        send["initial_files"]
    );
    drop(store);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn preflight_refusal_does_not_fence_or_consume_and_known_failure_consumes_once() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct Refuse;
    impl Engine for Refuse {
        fn admit(&mut self, _: &Value, _: &StepHost) -> coglet_runtime::Result<()> {
            Err("resource_exhausted: admission refused".into())
        }
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
            panic!("refused module entered guest")
        }
    }
    let send = setup();
    let directory = std::env::temp_dir().join(format!("coglet-preflight-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    assert!(service.run_next(&send["inputs"], &mut Refuse).is_err());
    assert_eq!(service.queued(), 1);
    assert!(service.records().is_empty());
    drop(service);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    let mut script = send["guest_script"].clone();
    script["termination"] = json!("aborted");
    script["failure"] = json!({"kind":"guest_return","error":{"code":"invalid_argument","message":"nonzero handle","retryable":false}});
    let result = service
        .run_next(&send["inputs"], &mut MockEngine::new(script))
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["status"], "aborted");
    assert_eq!(
        result["journal"]["input_hash"],
        result["journal"]["output_hash"]
    );
    assert_eq!(service.queued(), 0);
    drop(service);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    assert!(
        service
            .run_next(&send["inputs"], &mut Refuse)
            .unwrap()
            .is_none()
    );
    assert_eq!(service.records().len(), 1);
    drop(service);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn standalone_runtime_cannot_clear_ambiguous_attempt_by_sealing() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct Lost;
    impl Engine for Lost {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
            Err("lost".into())
        }
    }
    let send = setup();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    assert!(runtime.step(&send["inputs"], &mut Lost).is_err());
    let change = json!({"actor":format!("hub:{}","3".repeat(64)),"target":send["manifest"]["id"],"audit_id":"d".repeat(32),
        "ticket":null,"method":"debug.write","path":"/data/count","before":"30","after":"31"});
    assert!(
        runtime
            .seal_boundary(&send["inputs"]["trace_id"], 20, &[change])
            .is_err()
    );
    assert!(runtime.start_segment(&json!("e".repeat(32))).is_err());
}

#[test]
fn attempted_private_batch_survives_reopen_as_reconciliation_only_not_delivery() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct Lost;
    impl Engine for Lost {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
            Err("lost".into())
        }
    }
    let send = setup();
    let directory =
        std::env::temp_dir().join(format!("coglet-private-attempt-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    let event = send["inputs"]["events"][0].clone();
    service.enqueue(event.clone()).unwrap();
    drop(service);
    // Capture a persisted lease-bound event at the crash seam. Reopen must
    // revoke its lease, but retain the attempted prefix solely for reconciliation.
    let mut store = DurableStore::open(&path).unwrap();
    let mut state = store.state().unwrap().clone();
    state["private_events"] =
        json!({"0":{"surface":event["source"],"epoch":1,"expires":1000,"event":event}});
    state["surface_epochs"][event["source"].as_str().unwrap()] = json!(1);
    state["surface_sequences"][event["source"].as_str().unwrap()] =
        json!([event["seq"], event["logical_time"]]);
    state["runtime"]["unresolved_attempt"] = send["inputs"].clone();
    store.commit(&state, None).unwrap();
    drop(store);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    assert_eq!(
        service.queued(),
        1,
        "restart erased the attempted reservation"
    );
    assert!(service.run_next(&send["inputs"], &mut Lost).is_err());
    let observed = Runtime::from_fixture(&send)
        .unwrap()
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    service.recover_attempt(&observed["journal"]).unwrap();
    assert_eq!(service.queued(), 0);
    drop(service);
    drop(RuntimeService::open(&path, &send, 4).unwrap());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn initial_inbox_wakes_are_seeded_and_missing_recovered_wakes_are_rejected() {
    let mut send = setup();
    let message = json!({"id":"a".repeat(32),"index":0,"sender":send["inputs"]["events"][0]["source"],"event":send["inputs"]["events"][0]});
    send["initial_files"]["/data/inbox.jsonl"] = json!(hex::encode(format!(
        "{}\n",
        serde_json::to_string(&message).unwrap()
    )));
    send["initial_inbox_wakes"] = json!([0]);
    let directory =
        std::env::temp_dir().join(format!("coglet-initial-wakes-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let service = RuntimeService::open(&path, &send, 4).unwrap();
    assert_eq!(service.queued(), 1, "initial pending wake not scheduled");
    drop(service);
    let mut store = DurableStore::open(&path).unwrap();
    let mut state = store.state().unwrap().clone();
    state["router"]["queue"] = json!([]);
    store.commit(&state, None).unwrap();
    drop(store);
    assert!(
        RuntimeService::open(&path, &send, 4).is_err(),
        "pending wake silently lost"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn recovery_validates_private_lease_binding_before_restart_cleanup() {
    let send = setup();
    let event = send["inputs"]["events"][0].clone();
    for defect in ["ordinal", "event", "epoch", "highwater", "expiry"] {
        let directory =
            std::env::temp_dir().join(format!("coglet-private-{defect}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime.wal");
        let mut service = RuntimeService::open(&path, &send, 4).unwrap();
        service.enqueue(event.clone()).unwrap();
        drop(service);
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        state["private_events"] =
            json!({"0":{"surface":event["source"],"epoch":1,"expires":1000,"event":event}});
        state["surface_epochs"][event["source"].as_str().unwrap()] = json!(1);
        state["surface_sequences"][event["source"].as_str().unwrap()] =
            json!([event["seq"], event["logical_time"]]);
        match defect {
            "ordinal" => state["router"]["queue"] = json!([]),
            "event" => {
                state["private_events"]["0"]["event"]["correlation_id"] = json!("9".repeat(32))
            }
            "epoch" => state["private_events"]["0"]["epoch"] = json!(2),
            "highwater" => state["surface_sequences"] = json!({}),
            _ => state["private_events"]["0"]["expires"] = json!(0),
        }
        store.commit(&state, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, &send, 4).is_err(),
            "invalid {defect} lease silently purged"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn trace_identity_cannot_change_between_steps_or_on_trailing_boundary() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct MustNotRun;
    impl Engine for MustNotRun {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
            panic!("changed trace reached guest");
        }
    }
    let send = setup();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let first = runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let mut inputs = send["inputs"].clone();
    inputs["step"] = json!(1);
    inputs["previous"] = first["record_hash"].clone();
    inputs["trace_id"] = json!("c".repeat(32));
    assert!(runtime.step(&inputs, &mut MustNotRun).is_err());
    let change = json!({"actor":format!("hub:{}","3".repeat(64)),"target":send["manifest"]["id"],"audit_id":"d".repeat(32),
        "ticket":null,"method":"debug.write","path":"/data/count","before":"30","after":"31"});
    assert!(
        runtime
            .seal_boundary(&inputs["trace_id"], 20, &[change])
            .is_err()
    );
}

#[test]
fn recovered_attempt_revalidates_effective_budget_and_boundary_before_cleanup() {
    let send = setup();
    for defect in ["budget", "boundary", "batch"] {
        let directory = std::env::temp_dir().join(format!(
            "coglet-attempt-invalid-{defect}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime.wal");
        let mut service = RuntimeService::open(&path, &send, 4).unwrap();
        service
            .enqueue(send["inputs"]["events"][0].clone())
            .unwrap();
        drop(service);
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        let mut inputs = send["inputs"].clone();
        match defect {
            "budget" => {
                inputs["budgets"]["compute"] =
                    json!(send["manifest"]["budgets"]["compute"].as_u64().unwrap() + 1)
            }
            "boundary" => inputs["boundary"] = json!([{}]),
            _ => inputs["budgets"]["max_events"] = json!(0),
        }
        state["runtime"]["unresolved_attempt"] = inputs;
        store.commit(&state, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, &send, 4).is_err(),
            "invalid {defect} attempt accepted"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn recovered_source_reservations_cannot_have_gaps_or_reuse_consumed_sequences() {
    for defect in ["gap", "reuse"] {
        let mut send = setup();
        send["initial_timers"] = json!([{"id":"a".repeat(32),"at":1},{"id":"b".repeat(32),"at":2}]);
        send["initial_effect_ids"]["timers"] = json!(["a".repeat(32), "b".repeat(32)]);
        let directory =
            std::env::temp_dir().join(format!("coglet-source-{defect}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("wal");
        let mut service = RuntimeService::open(&path, &send, 4).unwrap();
        service.queue_due_timers(1).unwrap();
        let mut inputs = send["inputs"].clone();
        inputs["logical_time"] = json!(1);
        service
            .run_next(&inputs, &mut MockEngine::new(send["guest_script"].clone()))
            .unwrap();
        service.queue_due_timers(2).unwrap();
        drop(service);
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        let seq = if defect == "gap" { 998 } else { 0 };
        state["router"]["queue"][0][1]["seq"] = json!(seq);
        state["runtime"]["next_source_seq"] = json!(seq + 1);
        store.commit(&state, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, &send, 4).is_err(),
            "{defect} source reservation accepted"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn recovery_rejects_corrupt_retained_blobs() {
    let send = setup();
    let directory =
        std::env::temp_dir().join(format!("coglet-blob-corrupt-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    drop(RuntimeService::open(&path, &send, 4).unwrap());
    let mut store = DurableStore::open(&path).unwrap();
    let mut state = store.state().unwrap().clone();
    state["runtime"]["retained_blobs"] = json!({"a".repeat(64):"00"});
    store.commit(&state, None).unwrap();
    drop(store);
    assert!(
        RuntimeService::open(&path, &send, 4).is_err(),
        "corrupt content-addressed blob accepted"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn recovery_rejects_router_metadata_that_cannot_be_an_admitted_queue() {
    for (name, router) in [
        (
            "zero-capacity",
            json!({"capacity":0,"next_ordinal":0,"queue":[]}),
        ),
        (
            "ordinal-not-reserved",
            json!({"capacity":4,"next_ordinal":0,"queue":[[0,setup()["inputs"]["events"][0]]]}),
        ),
        (
            "duplicate-ordinal",
            json!({"capacity":4,"next_ordinal":1,"queue":[[0,setup()["inputs"]["events"][0]],[0,setup()["inputs"]["events"][0]]]}),
        ),
        (
            "invalid-event",
            json!({"capacity":4,"next_ordinal":1,"queue":[[0,{}]]}),
        ),
    ] {
        let send = setup();
        let directory =
            std::env::temp_dir().join(format!("coglet-router-{name}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime.wal");
        drop(RuntimeService::open(&path, &send, 4).unwrap());
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        state["router"] = router;
        store.commit(&state, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, &send, 4).is_err(),
            "invalid router {name} accepted"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn recovery_rejects_schema_valid_forged_runtime_wakes() {
    let send = setup();
    let owner = &send["manifest"]["id"];
    for (name, event) in [
        (
            "timer-without-request",
            json!({"source":owner,"kind":"runtime.timer","seq":0,"logical_time":10,"payload":{"id":"a".repeat(32),"at":10},"correlation_id":"a".repeat(32)}),
        ),
        (
            "inbox-without-line",
            json!({"source":owner,"kind":"runtime.inbox","seq":0,"logical_time":10,"payload":{"index":0,"id":"a".repeat(32)},"correlation_id":"a".repeat(32)}),
        ),
        (
            "direct-terminal",
            json!({"source":send["inputs"]["events"][0]["source"],"kind":"action.outcome","seq":0,"logical_time":10,"payload":{},"correlation_id":"a".repeat(32)}),
        ),
    ] {
        let directory =
            std::env::temp_dir().join(format!("coglet-wake-{name}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime.wal");
        drop(RuntimeService::open(&path, &send, 4).unwrap());
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        state["router"] = json!({"capacity":4,"next_ordinal":1,"queue":[[0,event]]});
        state["runtime"]["next_source_seq"] = json!(1);
        store.commit(&state, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, &send, 4).is_err(),
            "forged {name} accepted"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn effect_ledger_requires_sorted_unique_ids_before_deserialization() {
    for ids in [
        json!(["b".repeat(32), "a".repeat(32)]),
        json!(["a".repeat(32), "a".repeat(32)]),
    ] {
        let mut send = setup();
        send["initial_effect_ids"]["timers"] = ids;
        assert!(
            Runtime::from_fixture(&send).is_err(),
            "unordered/duplicate ledger silently normalized"
        );
    }
}

#[test]
fn recovery_binds_all_code_files_not_just_wasm_and_provenance() {
    let mut send = setup();
    send["initial_files"]["/code/prompts/system.txt"] = json!("68656c6c6f");
    let directory =
        std::env::temp_dir().join(format!("coglet-recovery-code-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    drop(RuntimeService::open(&path, &send, 4).unwrap());
    send["initial_files"]["/code/prompts/system.txt"] = json!("627965");
    assert!(
        RuntimeService::open(&path, &send, 4).is_err(),
        "changed auxiliary pinned code accepted"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn recovery_rejects_forged_host_state_even_with_valid_internal_frame_checksum() {
    for (name, field, value) in [
        (
            "pending",
            "pending_timers",
            json!({"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","at":10}}),
        ),
        ("source-highwater", "next_source_seq", json!(999)),
        ("wakes", "inbox_wakes", json!([99])),
        ("head", "previous", json!("a".repeat(64))),
        ("step", "step", json!(2)),
        (
            "extra-ledger",
            "ledger",
            json!({"actions":[],"timers":["a".repeat(32)],"llm_requests":[]}),
        ),
    ] {
        let send = setup();
        let directory =
            std::env::temp_dir().join(format!("coglet-recovery-{name}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime.wal");
        let mut service = RuntimeService::open(&path, &send, 4).unwrap();
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
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        state["runtime"][field] = value;
        store.commit(&state, None).unwrap();
        drop(store);
        assert!(
            RuntimeService::open(&path, &send, 4).is_err(),
            "invalid recovered {name} accepted"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
