//! Persistence-seam controls; actual daemon/guest proof is in device_lifecycle.
use super::*;

fn context(id: char) -> StartRecovery {
    StartRecovery {
        operation_id: id.to_string().repeat(32),
        intent: id.to_string().repeat(64),
        scope: ReceiptScope {
            actor: "restart-test".into(),
            ticket: None,
            method: "hub.start".into(),
        },
        grant_generation: 7,
        delegation_generation: 9,
        delegation: json!({"statement":{"generation":9}}),
    }
}

#[test]
fn restart_context_and_fault_commit_with_receipts_and_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = super::tests::fixture("no-effects");
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let identity = json!({"initial":{"statement":{"new":"a".repeat(40)}}});
    service.managed_install(&identity, &"1".repeat(32)).unwrap();
    let original = context('2');
    let receipt = service.managed_start_authorized(&original).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(
        service.managed_recovery().unwrap().operation_id,
        original.operation_id
    );
    service.managed_recovery_fault().unwrap();
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.managed_status()["state"], "faulted");
    assert!(!service.managed_running());
    assert_eq!(
        service.managed_start_authorized(&original).unwrap(),
        receipt
    );
    assert_eq!(service.managed_status()["state"], "faulted");
    service.managed_start_authorized(&context('3')).unwrap();
    assert_eq!(service.managed_status()["state"], "running");
    assert_eq!(
        service.managed_recovery().unwrap().operation_id,
        "3".repeat(32)
    );
}

#[test]
fn restart_context_missing_or_not_bound_to_original_receipt_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let setup = super::tests::fixture("no-effects");
    let mut service = RuntimeService::open(&dir.path().join("state.wal"), &setup, 8).unwrap();
    service
        .managed_install(&json!({"live_commit":"a".repeat(40)}), &"1".repeat(32))
        .unwrap();
    // Legacy starts have no stored authority and cannot automatically resume.
    service.managed_start().unwrap();
    assert!(service.managed_recovery().is_err());
    let original = context('2');
    service.managed_start_authorized(&original).unwrap();
    service
        .state
        .managed
        .as_mut()
        .unwrap()
        .recovery
        .as_mut()
        .unwrap()
        .intent = "f".repeat(64);
    assert!(service.managed_recovery().is_err());
    service.state.managed.as_mut().unwrap().recovery = Some(original);
    service
        .state
        .managed
        .as_mut()
        .unwrap()
        .recovery
        .as_mut()
        .unwrap()
        .scope
        .actor = "other".into();
    assert!(service.managed_recovery().is_err());
}

#[test]
fn restart_missing_context_faults_until_fresh_authorized_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = super::tests::fixture("no-effects");
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    service
        .managed_install(&json!({"live_commit":"a".repeat(40)}), &"1".repeat(32))
        .unwrap();
    service.managed_start_authorized(&context('2')).unwrap();
    // Write an actual older-format frame: the new optional fields are absent,
    // not null. Recovery must never infer authority from the retained receipt.
    let mut legacy = serde_json::to_value(&service.state).unwrap();
    let managed = legacy["managed"].as_object_mut().unwrap();
    managed.remove("recovery");
    managed.remove("recovery_fault");
    service.store.commit(&legacy, None).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert!(
        service
            .managed_recovery()
            .err()
            .unwrap()
            .contains("missing start recovery")
    );
    service.managed_recovery_fault().unwrap();
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.managed_status()["state"], "faulted");
    service.managed_start_authorized(&context('2')).unwrap();
    assert_eq!(service.managed_status()["state"], "faulted");
    service.managed_start_authorized(&context('3')).unwrap();
    assert_eq!(
        service.managed_recovery().unwrap().operation_id,
        "3".repeat(32)
    );
}

#[test]
fn restart_unresolved_attempt_and_pending_stop_prevent_resume() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = super::tests::fixture("no-effects");
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    service
        .managed_install(&json!({"live_commit":"a".repeat(40)}), &"1".repeat(32))
        .unwrap();
    service.managed_start_authorized(&context('2')).unwrap();
    service.state.runtime.unresolved_attempt = Some(setup["inputs"].clone());
    assert!(service.managed_recovery().is_err());
    // Stop records its fence without guessing an unknown attempt outcome.
    service.managed_stop(100).unwrap();
    assert!(!service.managed_running());
    assert!(service.state.managed.as_ref().unwrap().cancellation_pending);
    assert!(service.managed_recovery().is_err());
    assert!(service.state.runtime.unresolved_attempt.is_some());
    assert!(service.managed_start_authorized(&context('3')).is_err());
}

#[test]
fn restart_code_validation_compares_recovered_bytes_to_verified_archive() {
    let dir = tempfile::tempdir().unwrap();
    let setup = super::tests::fixture("no-effects");
    let service = RuntimeService::open(&dir.path().join("state.wal"), &setup, 8).unwrap();
    service.managed_verify_code(&setup).unwrap();
    let mut different = setup.clone();
    different["initial_files"]["/code/bin/step.wasm"] = json!("00");
    assert!(service.managed_verify_code(&different).is_err());
}
