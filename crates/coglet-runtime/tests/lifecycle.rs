use coglet_runtime::{MockEngine, Runtime, RuntimeService};
use serde_json::{Value, json};
use std::fs;

fn fixture(name: &str) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()["steps"][0]["send"]
        .clone()
}

fn next_input(send: &Value, previous: &Value, event: Value) -> Value {
    let mut inputs = send["inputs"].clone();
    inputs["step"] = json!(1);
    inputs["previous"] = previous["record_hash"].clone();
    inputs["logical_time"] = json!(1_000_000);
    inputs["events"] = json!([event]);
    inputs["boundary"] = json!([]);
    inputs
}

fn receipt_change(send: &Value, action: &Value, kind: &str) -> (Value, Value) {
    let payload = if kind == "runtime.action_rejected" {
        json!({"id":action["id"],"correlation_id":action["correlation_id"],"accepted":false,
            "error":{"code":"permission_denied","message":"revoked","retryable":false}})
    } else {
        json!({"id":action["id"],"correlation_id":action["correlation_id"],"status":"completed","result":{}})
    };
    let source = if kind == "runtime.action_rejected" {
        send["manifest"]["id"].clone()
    } else {
        send["inputs"]["events"][0]["source"].clone()
    };
    let message = json!({"id":"e".repeat(32),"index":0,"sender":source,
        "event":{"source":source,"kind":kind,"seq":1,"logical_time":20,
            "payload":payload,"correlation_id":action["correlation_id"]}});
    let mut line = serde_json::to_vec(&message).unwrap();
    line.push(b'\n');
    let change = json!({"actor":send["manifest"]["id"],"target":send["manifest"]["id"],
        "audit_id":"f".repeat(32),"ticket":null,"method":"runtime.receipt",
        "path":"/data/inbox.jsonl","before":null,"after":hex::encode(line)});
    let wake = json!({"source":send["manifest"]["id"],"kind":"runtime.inbox","seq":2,
        "logical_time":20,"correlation_id":action["correlation_id"],"payload":{"index":0,"id":message["id"]}});
    (change, wake)
}

#[test]
fn terminal_rejection_consumes_pending_action_once_and_keeps_lifetime_id() {
    let send = fixture("action");
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let first = runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let action = &send["guest_script"]["calls"][0]["argument"];
    let (change, wake) = receipt_change(&send, action, "runtime.action_rejected");
    let mut inputs = next_input(&send, &first, wake);
    inputs["boundary"] = json!([change]);
    let second = runtime
        .step(
            &inputs,
            &mut MockEngine::new(fixture("no-effects")["guest_script"].clone()),
        )
        .unwrap();
    assert_eq!(
        second["journal"]["boundary"][0]["method"],
        "runtime.receipt"
    );
    let mut third = next_input(&send, &second, send["inputs"]["events"][0].clone());
    third["step"] = json!(2);
    let mut duplicate = send["guest_script"].clone();
    duplicate["calls"][0]["result"] = json!(4);
    let result = runtime
        .step(&third, &mut MockEngine::new(duplicate))
        .unwrap();
    assert!(result["journal"]["actions"].as_array().unwrap().is_empty());
}

#[test]
fn durable_terminal_append_uses_shared_source_sequence_and_survives_restart() {
    use coglet_runtime::DurableStore;
    let send = fixture("action");
    let directory =
        std::env::temp_dir().join(format!("coglet-terminal-seal-{}", std::process::id()));
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
    let (change, _) = receipt_change(
        &send,
        &send["guest_script"]["calls"][0]["argument"],
        "runtime.action_rejected",
    );
    let record = service
        .seal_boundary(
            &send["inputs"]["trace_id"],
            20,
            std::slice::from_ref(&change),
        )
        .unwrap();
    assert_eq!(service.queued(), 1);
    drop(service);
    let store = DurableStore::open(&path).unwrap();
    let event = &store.state().unwrap()["router"]["queue"][0][1];
    assert_eq!(event["kind"], "runtime.inbox");
    assert_eq!(
        event["seq"], 2,
        "wake reused or regressed terminal event's shared source seq"
    );
    assert!(
        store.state().unwrap()["runtime"]["pending_actions"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    drop(store);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    assert_eq!(service.records()[1], record);
    assert!(
        service
            .seal_boundary(&send["inputs"]["trace_id"], 20, &[change])
            .is_err()
    );
    assert_eq!(service.queued(), 1);
    assert_eq!(service.records().len(), 2);
    drop(service);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn receipt_for_unknown_or_aborted_action_is_rejected() {
    let mut send = fixture("action");
    send["guest_script"]["termination"] = json!("aborted");
    send["guest_script"]["failure"] = fixture("abort-wall")["guest_script"]["failure"].clone();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let first = runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let (change, wake) = receipt_change(
        &send,
        &send["guest_script"]["calls"][0]["argument"],
        "runtime.action_rejected",
    );
    let mut inputs = next_input(&send, &first, wake);
    inputs["boundary"] = json!([change]);
    assert!(
        runtime
            .step(
                &inputs,
                &mut MockEngine::new(fixture("no-effects")["guest_script"].clone())
            )
            .is_err()
    );
}

fn timer_event(send: &Value) -> Value {
    let timer = &send["guest_script"]["calls"][0]["argument"];
    json!({"source":send["manifest"]["id"], "kind":"runtime.timer", "seq":0,
        "logical_time":1_000_000,"payload":{"id":timer["id"],"at":timer["at"]},
        "correlation_id":timer["id"]})
}

#[test]
fn uncommitted_timer_cannot_be_delivered() {
    let send = fixture("timer");
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let mut inputs = send["inputs"].clone();
    inputs["events"] = json!([timer_event(&send)]);
    assert!(
        runtime
            .step(
                &inputs,
                &mut MockEngine::new(fixture("no-effects")["guest_script"].clone())
            )
            .is_err(),
        "invented timer reached guest"
    );
}

#[test]
fn timer_wake_requires_exact_pending_argument_and_is_consumed_once() {
    let send = fixture("timer");
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let result = runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    let event = timer_event(&send);
    let mut bad = event.clone();
    bad["payload"]["at"] = json!(999);
    let script = fixture("no-effects")["guest_script"].clone();
    assert!(
        runtime
            .step(
                &next_input(&send, &result, bad),
                &mut MockEngine::new(script.clone())
            )
            .is_err(),
        "changed timer payload accepted"
    );
    let second = runtime
        .step(
            &next_input(&send, &result, event.clone()),
            &mut MockEngine::new(script.clone()),
        )
        .unwrap();
    let mut third = next_input(&send, &second, event);
    third["step"] = json!(2);
    assert!(
        runtime.step(&third, &mut MockEngine::new(script)).is_err(),
        "timer fired twice"
    );
}

#[test]
fn past_due_timer_fires_after_commit_and_survives_restart_without_duplication() {
    let mut send = fixture("timer");
    send["guest_script"]["calls"][0]["argument"]["at"] = json!(0);
    let dir = std::env::temp_dir().join(format!("coglet-lifecycle-timer-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("runtime.wal");
    let mut service = RuntimeService::open(&path, &send, 8).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    let committed = service
        .run_next(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    assert_eq!(committed["journal"]["events"].as_array().unwrap().len(), 1);
    drop(service);
    let mut service = RuntimeService::open(&path, &send, 8).unwrap();
    let woke = service
        .run_next(
            &send["inputs"],
            &mut MockEngine::new(fixture("no-effects")["guest_script"].clone()),
        )
        .unwrap();
    assert!(woke.is_some(), "committed due timer was lost on restart");
    let woke = woke.unwrap();
    assert_eq!(woke["journal"]["step"], 1);
    assert_eq!(woke["journal"]["events"][0]["kind"], "runtime.timer");
    assert_eq!(woke["journal"]["events"][0]["payload"]["at"], 0);
    assert!(
        service
            .run_next(
                &send["inputs"],
                &mut MockEngine::new(fixture("no-effects")["guest_script"].clone())
            )
            .unwrap()
            .is_none()
    );
    drop(service);
    fs::remove_dir_all(dir).unwrap();
}
