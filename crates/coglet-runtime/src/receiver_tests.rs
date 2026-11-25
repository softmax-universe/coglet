use super::*;

#[path = "receiver_recovery_tests.rs"]
mod recovery;

fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["initial_files"]
        .as_object_mut()
        .unwrap()
        .retain(|p, _| !p.starts_with("/data/"));
    setup
}
fn request(setup: &Value, method: &str, id: u64, params: Value) -> Value {
    let mut request = json!({"method":method,"target":setup["manifest"]["id"],"correlation_id":format!("{id:032x}"),
        "timeout_ms":5000,"params":params});
    let actor = format!("hub:{}", "1".repeat(64));
    if matches!(method, "debug.write" | "hub.inbox") {
        request["params"]["actor"] = json!(actor);
    } else {
        request["context"] = json!({"actor":actor});
    }
    request
}
fn write(service: &RuntimeService, setup: &Value, id: u64, text: &str) -> Value {
    request(
        setup,
        "debug.write",
        id,
        json!({"path":"/data/note.txt","text":text,"expected_hash":service.receiver_snapshot().files.tree_hash().unwrap()}),
    )
}
fn inbox(setup: &Value, id: u64, message_id: u64) -> Value {
    let source = format!("hub:{}", "1".repeat(64));
    request(
        setup,
        "hub.inbox",
        id,
        json!({"message":{"id":format!("{message_id:032x}"),"sender":source,
        "event":{"source":source,"kind":"test.message","seq":message_id,"logical_time":0,"payload":{"text":"hello λ"},"correlation_id":format!("{message_id:032x}")}}}),
    )
}
fn service(path: &Path, setup: &Value, capacity: usize) -> RuntimeService {
    let mut service = RuntimeService::open(path, setup, capacity).unwrap();
    service
        .managed_install(&json!({"live_commit":"a".repeat(40)}), &"f".repeat(32))
        .unwrap();
    service
}

#[test]
fn receiver_inbox_preserves_selected_profile_across_stage_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let mut setup = setup();
    let mut existing = inbox(&setup, 90, 90)["params"]["message"].clone();
    existing["event"]["kind"] = json!("softmax.app.focus_changed");
    existing["index"] = json!(0);
    let mut raw = serde_json::to_vec(&existing).unwrap();
    raw.push(b'\n');
    setup["initial_files"]["/data/inbox.jsonl"] = json!(hex::encode(raw));
    assert!(RuntimeService::open(&path, &setup, 8).is_err());
    setup["contract_profile"] = json!("surface-0.2");
    // Installing a managed checkout requires an empty private namespace. The
    // inbox here is already present in the selected-profile runtime snapshot;
    // exercise staging directly rather than bypassing that install fence.
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    let request = inbox(&setup, 1, 100);
    let mut next = service.state.clone();
    RuntimeService::receiver_stage(&mut next, &request).unwrap();
    let files = next.staged_boundary.as_ref().unwrap().runtime.files.clone();
    assert!(
        crate::boundary::inbox_with_profile(&files, crate::ContractProfile::Surface01).is_err()
    );
    let messages = crate::boundary::inbox_with_profile(&files, service.contract_profile()).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], existing);
    assert_eq!(messages[1]["event"], request["params"]["message"]["event"]);
    RuntimeService::receiver_stage(&mut next, &request).unwrap();
    assert_eq!(next.staged_boundary.as_ref().unwrap().runtime.files, files);
    drop(service);
    let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(
        reopened.contract_profile(),
        crate::ContractProfile::Surface02
    );
    let retained = crate::boundary::inbox_with_profile(
        &reopened.receiver_snapshot().files,
        reopened.contract_profile(),
    )
    .unwrap();
    assert_eq!(retained, vec![existing]);
}

#[test]
fn receiver_append_preserves_selected_app_inbox_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let mut setup = setup();
    setup["contract_profile"] = json!("surface-0.2");
    let mut historical = inbox(&setup, 1, 100)["params"]["message"].clone();
    historical["index"] = json!(0);
    historical["event"]["kind"] = json!("softmax.app.view_changed");
    let mut raw = serde_json::to_vec(&historical).unwrap();
    raw.push(b'\n');
    setup["initial_files"]["/data/inbox.jsonl"] = json!(hex::encode(raw));
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    // The hub request remains frozen/two-component. Only retained local inbox
    // parsing uses the independently provisioned runtime profile.
    let append = inbox(&setup, 2, 101);
    service.receiver_admit(&append).unwrap();
    service
        .receiver_apply(append["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    assert_eq!(service.receiver_admit(&append).unwrap()["state"], "applied");
    let runtime = service.receiver_snapshot();
    let messages =
        crate::boundary::inbox_with_profile(&runtime.files, runtime.contract_profile()).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], historical);
    assert_eq!(messages[1]["event"], append["params"]["message"]["event"]);
    assert!(
        crate::boundary::inbox_with_profile(&runtime.files, crate::ContractProfile::Surface01)
            .is_err()
    );
    drop(service);
    let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    let runtime = reopened.receiver_snapshot();
    assert_eq!(
        crate::boundary::inbox_with_profile(&runtime.files, runtime.contract_profile()).unwrap(),
        messages
    );
}

#[test]
fn debug_write_queued_then_committed_snapshot_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = setup();
    let mut service = service(&path, &setup, 8);
    let request = write(&service, &setup, 1, "hello λ");
    let id = request["correlation_id"].as_str().unwrap();
    let queued = service.receiver_admit(&request).unwrap();
    assert_eq!(queued["state"], "queued");
    assert_eq!(
        service.receiver_read("/data/note.txt", 0, 64).unwrap_err(),
        "not_found"
    );
    drop(service);
    let mut reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(reopened.receiver_pending_len(), 1);
    reopened.receiver_apply(id, &|| Ok(())).unwrap();
    assert_eq!(
        reopened.receiver_admit(&request).unwrap()["state"],
        "applied"
    );
    assert_eq!(
        reopened.receiver_read("/data/note.txt", 0, 64).unwrap()["text"],
        "hello λ"
    );
    assert!(
        reopened.records().is_empty(),
        "applying a back-channel write must not seal the trace"
    );
    drop(reopened);
    let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    let read = reopened.receiver_read("/data/note.txt", 0, 7).unwrap();
    assert_eq!(read["text"], "hello ");
    assert_eq!(read["next_offset"], 6);
    assert_eq!(
        reopened.receiver_read("/data/note.txt", 7, 64).unwrap_err(),
        "invalid_argument"
    );
    assert_eq!(reopened.state.receiver.audit.len(), 2);
}

#[test]
fn debug_write_cas_is_rechecked_at_application_and_failure_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let setup = setup();
    let mut service = service(&dir.path().join("state.wal"), &setup, 8);
    let first = write(&service, &setup, 1, "first");
    let second = write(&service, &setup, 2, "second");
    service.receiver_admit(&first).unwrap();
    service.receiver_admit(&second).unwrap();
    service
        .receiver_apply(first["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    service
        .receiver_apply(second["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    assert_eq!(
        service.receiver_admit(&second).unwrap()["error"]["code"],
        "conflict"
    );
    assert_eq!(
        service.receiver_read("/data/note.txt", 0, 64).unwrap()["text"],
        "first"
    );
    assert_eq!(
        service.state.receiver.audit[1]["before_hash"],
        service.state.receiver.audit[1]["after_hash"]
    );
    for path in [
        "/data/inbox.jsonl",
        "/data/runtime/cursor",
        "/data/../data/note.txt",
        "/data//x",
        "/data/x\\y",
        "/code/a",
    ] {
        let mut bad = write(&service, &setup, 3, "bad");
        bad["params"]["path"] = json!(path);
        assert!(service.receiver_admit(&bad).is_err(), "{path}");
    }
}

#[test]
fn inbox_queue_receipt_and_wake_commit_together_and_dedup_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = setup();
    let mut service = service(&path, &setup, 8);
    let first = inbox(&setup, 1, 100);
    service.receiver_admit(&first).unwrap();
    assert_eq!(service.queued(), 0);
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    service
        .receiver_apply(first["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    assert_eq!(service.queued(), 1);
    let messages = crate::boundary::inbox_with_profile(
        &service.receiver_snapshot().files,
        service.contract_profile(),
    )
    .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["index"], 0);
    assert_eq!(messages[0]["event"], first["params"]["message"]["event"]);
    let mut changed = inbox(&setup, 2, 100);
    changed["params"]["message"]["event"]["payload"] = json!({"text":"different"});
    assert_eq!(service.receiver_admit(&changed).unwrap_err(), "conflict");
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let duplicate = inbox(&setup, 2, 100);
    service.receiver_admit(&duplicate).unwrap();
    service
        .receiver_apply(duplicate["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    assert_eq!(service.queued(), 1);
    assert_eq!(
        crate::boundary::inbox_with_profile(
            &service.receiver_snapshot().files,
            service.contract_profile()
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn receipt_ownership_cancel_and_revocation_do_not_change_files() {
    let dir = tempfile::tempdir().unwrap();
    let setup = setup();
    let mut service = service(&dir.path().join("state.wal"), &setup, 8);
    let request = write(&service, &setup, 1, "secret");
    service.receiver_admit(&request).unwrap();
    let id = request["correlation_id"].as_str().unwrap();
    let scope = receiver_scope(&request);
    assert!(service.receiver_receipt_for(id, "another", None).is_none());
    assert!(
        service
            .receiver_receipt_for(id, &scope.actor, Some(&"a".repeat(32)))
            .is_none()
    );
    assert_eq!(
        service.receiver_cancel(id, "another", None).unwrap_err(),
        "not_found"
    );
    assert_eq!(
        service.receiver_cancel(id, &scope.actor, None).unwrap()["state"],
        "cancelled"
    );
    assert_eq!(
        service.receiver_cancel(id, &scope.actor, None).unwrap_err(),
        "conflict"
    );
    let second = write(&service, &setup, 2, "secret");
    service.receiver_admit(&second).unwrap();
    service
        .receiver_apply(second["correlation_id"].as_str().unwrap(), &|| {
            Err("permission_denied".into())
        })
        .unwrap();
    assert_eq!(
        service.receiver_admit(&second).unwrap()["error"]["code"],
        "permission_denied"
    );
    assert_eq!(
        service.receiver_read("/data/note.txt", 0, 64).unwrap_err(),
        "not_found"
    );
}

#[test]
fn inbox_backpressure_commits_no_partial_append_or_wake() {
    let dir = tempfile::tempdir().unwrap();
    let setup = setup();
    let mut service = service(&dir.path().join("state.wal"), &setup, 1);
    let first = inbox(&setup, 1, 100);
    let second = inbox(&setup, 2, 101);
    service.receiver_admit(&first).unwrap();
    service.receiver_admit(&second).unwrap();
    service
        .receiver_apply(first["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    let before = service.receiver_snapshot().files.tree_hash().unwrap();
    service
        .receiver_apply(second["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    assert_eq!(
        service.receiver_admit(&second).unwrap()["error"]["code"],
        "resource_exhausted"
    );
    assert_eq!(
        service.receiver_snapshot().files.tree_hash().unwrap(),
        before
    );
    assert_eq!(service.queued(), 1);
}

#[test]
fn cancellation_original_request_id_is_durable_and_cannot_cancel_a_different_intent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = setup();
    let mut service = service(&path, &setup, 8);
    let first = inbox(&setup, 1, 100);
    let second = inbox(&setup, 2, 101);
    service.receiver_admit(&first).unwrap();
    service.receiver_admit(&second).unwrap();
    let cancel = request(
        &setup,
        "hub.cancel",
        3,
        json!({"operation_id":first["correlation_id"]}),
    );
    let receipt = service.receiver_cancel_request(&cancel).unwrap();
    assert_eq!(receipt["state"], "cancelled");
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.receiver_cancel_request(&cancel).unwrap(), receipt);
    let mut changed = cancel.clone();
    changed["params"]["operation_id"] = second["correlation_id"].clone();
    assert_eq!(
        service.receiver_cancel_request(&changed).unwrap_err(),
        "conflict"
    );
    let mut collision = cancel.clone();
    collision["correlation_id"] = second["correlation_id"].clone();
    assert_eq!(
        service.receiver_cancel_request(&collision).unwrap_err(),
        "conflict"
    );
    let collision = inbox(&setup, 3, 102);
    assert_eq!(service.receiver_admit(&collision).unwrap_err(), "conflict");
    assert_eq!(service.receiver_pending_len(), 1);
    assert_eq!(service.queued(), 0);
}

#[test]
fn inbox_id_from_another_append_path_cannot_hide_different_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut setup = setup();
    let original = inbox(&setup, 1, 100)["params"]["message"].clone();
    let mut line = original.clone();
    line["index"] = json!(0);
    let mut raw = serde_json::to_vec(&line).unwrap();
    raw.push(b'\n');
    setup["initial_files"]["/data/inbox.jsonl"] = json!(hex::encode(raw));
    let mut service = RuntimeService::open(&dir.path().join("state.wal"), &setup, 8).unwrap();
    let mut changed = inbox(&setup, 2, 100);
    changed["params"]["message"]["event"]["payload"] = json!({"different":true});
    service.receiver_admit(&changed).unwrap();
    service
        .receiver_apply(changed["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    assert_eq!(
        service.receiver_admit(&changed).unwrap()["error"]["code"],
        "conflict"
    );
    let messages = crate::boundary::inbox_with_profile(
        &service.receiver_snapshot().files,
        service.contract_profile(),
    )
    .unwrap();
    assert_eq!(messages, vec![line]);
}
