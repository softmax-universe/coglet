use coglet_runtime::{MockEngine, Runtime, Snapshot, cbor, digest};
use serde_json::{Value, json};

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
fn heard(played: u64) -> Value {
    json!({"items":[{"surface":format!("surface:{}","2".repeat(64)),"item":"a".repeat(32),
        "duration_ms":20,"played_ms":played,"stopped":false,
        "spans":[{"end_ms":10,"text":"hello"},{"end_ms":20,"text":" world"}],
        "heard":if played >= 20 { "hello world" } else if played >= 10 { "hello" } else { "" }}]})
}
fn change(send: &Value, before: Option<&Value>, after: &Value) -> Value {
    json!({"actor":send["manifest"]["id"],"target":send["manifest"]["id"],
        "audit_id":"b".repeat(32),"ticket":null,"method":"runtime.heard",
        "path":"/data/runtime/heard.cbor","before":before.map(|v|hex::encode(cbor(v).unwrap())),
        "after":hex::encode(cbor(after).unwrap())})
}
#[test]
fn heard_boundary_is_hash_chained_and_replayed_without_surface_access() {
    let send = fixture();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let mut inputs = send["inputs"].clone();
    inputs["boundary"] = json!([change(&send, None, &heard(10))]);
    let first = runtime
        .step(&inputs, &mut MockEngine::new(send["guest_script"].clone()))
        .unwrap();
    assert_eq!(first["journal"]["calls"], json!([]));
    assert_eq!(first["journal"]["events"], send["inputs"]["events"]);
    assert_ne!(
        first["journal"]["before_hash"],
        first["journal"]["input_hash"]
    );
    let mut unhashed = first["journal"].clone();
    unhashed.as_object_mut().unwrap().remove("record_hash");
    assert_eq!(first["record_hash"], digest(&cbor(&unhashed).unwrap()));
    inputs["step"] = json!(1);
    inputs["previous"] = first["record_hash"].clone();
    inputs["boundary"] = json!([change(&send, Some(&heard(10)), &heard(20))]);
    let second = runtime
        .step(&inputs, &mut MockEngine::new(send["guest_script"].clone()))
        .unwrap();
    let mut replay = Runtime::from_fixture(&send).unwrap();
    assert_eq!(
        replay
            .replay(
                runtime.records(),
                &mut MockEngine::new(send["guest_script"].clone())
            )
            .unwrap(),
        vec![
            first["state_hash"].as_str().unwrap(),
            second["state_hash"].as_str().unwrap()
        ]
    );
}
#[test]
fn zero_step_heard_boundary_exports_and_replays_without_invoking_engine() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct MustNotRun;
    impl Engine for MustNotRun {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
            panic!("boundary export invoked guest engine");
        }
    }
    let send = fixture();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let boundary = runtime
        .seal_boundary(
            &send["inputs"]["trace_id"],
            20,
            &[change(&send, None, &heard(10))],
        )
        .unwrap();
    assert!(boundary.get("step").is_none());
    assert_eq!(boundary["previous"], Value::Null);
    assert_eq!(
        boundary["output_files"]["/data/runtime/heard.cbor"],
        hex::encode(cbor(&heard(10)).unwrap())
    );
    let mut replay = Runtime::from_fixture(&send).unwrap();
    assert_eq!(
        replay
            .replay(std::slice::from_ref(&boundary), &mut MustNotRun)
            .unwrap(),
        vec![boundary["output_hash"].as_str().unwrap()]
    );
    assert!(
        runtime.step(&send["inputs"], &mut MustNotRun).is_err(),
        "sealed trace accepted later guest step"
    );
}

#[test]
fn durable_zero_step_heard_seal_survives_reopening_without_queueing_wake() {
    use coglet_runtime::RuntimeService;
    let send = fixture();
    let directory = std::env::temp_dir().join(format!("coglet-heard-seal-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    let boundary = service
        .seal_boundary(
            &send["inputs"]["trace_id"],
            20,
            &[change(&send, None, &heard(10))],
        )
        .unwrap();
    assert_eq!(service.queued(), 0);
    drop(service);
    let service = RuntimeService::open(&path, &send, 4).unwrap();
    assert_eq!(service.records(), &[boundary]);
    assert_eq!(service.queued(), 0);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn initial_heard_projection_is_validated_not_trusted() {
    let mut send = fixture();
    let mut state = heard(10);
    state["items"][0]["heard"] = json!("hello world");
    send["initial_files"]["/data/runtime/heard.cbor"] = json!(hex::encode(cbor(&state).unwrap()));
    assert!(
        Runtime::from_fixture(&send).is_err(),
        "unheard text admitted into history"
    );
}
#[test]
fn heard_monotonicity_rejects_rewritten_alignment() {
    let send = fixture();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let mut inputs = send["inputs"].clone();
    inputs["boundary"] = json!([change(&send, None, &heard(10))]);
    let first = runtime
        .step(&inputs, &mut MockEngine::new(send["guest_script"].clone()))
        .unwrap();
    inputs["step"] = json!(1);
    inputs["previous"] = first["record_hash"].clone();
    let mut changed = heard(20);
    changed["items"][0]["spans"][0]["text"] = json!("bye");
    changed["items"][0]["heard"] = json!("bye world");
    inputs["boundary"] = json!([change(&send, Some(&heard(10)), &changed)]);
    assert!(
        runtime
            .step(&inputs, &mut MockEngine::new(send["guest_script"].clone()))
            .is_err()
    );
}
#[test]
fn reserved_file_descendants_and_ancestors_cannot_be_guest_files() {
    for path in [
        "/data/runtime",
        "/data/runtime/heard.cbor/child",
        "/data/inbox.jsonl/child",
        "/logs/events.jsonl/child",
    ] {
        let mut files = Snapshot::new(Default::default()).unwrap();
        assert!(
            files.guest_write(path, b"x").is_err(),
            "guest shadowed reserved file with {path}"
        );
    }
}
