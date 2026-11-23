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
