//! Coordinator fault seams, separate from native-engine receiver qualification.
use super::*;

fn prepared(service: &mut RuntimeService, setup: &Value) -> Value {
    let pause = request(setup, "debug.pause", 50, json!({}));
    service.receiver_admit(&pause).unwrap();
    service
        .receiver_apply(pause["correlation_id"].as_str().unwrap(), &|| Ok(()))
        .unwrap();
    let step = request(setup, "debug.step", 51, json!({}));
    service.receiver_admit(&step).unwrap();
    let mut next = service.state.clone();
    next.receiver.active_step = Some(step["correlation_id"].as_str().unwrap().into());
    let seq = next.runtime.next_source_seq;
    next.runtime.next_source_seq += 1;
    next.router
        .enqueue(
            json!({"source":setup["manifest"]["id"],"kind":"runtime.debug_step","seq":seq,
        "logical_time":0,"payload":{},"correlation_id":step["correlation_id"]}),
        )
        .unwrap();
    service.commit_receiver(next).unwrap();
    step
}

#[cfg(feature = "native-engine")]
#[test]
fn detached_receiver_code_pending_blocks_prepare_and_live_entry_across_reopen() {
    struct NoEntry;
    impl Engine for NoEntry {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
            panic!("code admission must fence guest entry")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = setup();
    let mut original = service(&path, &setup, 8);
    prepared(&mut original, &setup);
    original.debug_executing = true;
    original.fence_code(true).unwrap();
    assert_eq!(
        original.prepare_detached(&setup["inputs"]).err().unwrap(),
        "installed code application pending"
    );
    assert!(!original.detached_execution);
    original.fence_code(false).unwrap();
    let run = original
        .prepare_detached(&setup["inputs"])
        .unwrap()
        .unwrap();
    // SQL push admission can arrive while native admission owns the engine but
    // has not yet persisted the unresolved-attempt WAL fence.
    original.fence_code(true).unwrap();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(original));
    assert_eq!(
        RuntimeService::execute_detached(&shared, run, &mut NoEntry, &|f| f(), &|_, _| panic!(
            "pending code must refuse before live gate"
        ))
        .unwrap_err(),
        "installed code application pending"
    );
    let locked = shared.lock().unwrap();
    assert!(locked.state.runtime.unresolved_attempt.is_none());
    assert!(locked.records().is_empty());
    assert!(locked.detached_execution);
    drop(locked);
    drop(shared);
    let mut reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert!(reopened.code_pending());
    assert_eq!(
        reopened.prepare_detached(&setup["inputs"]).err().unwrap(),
        "installed code application pending"
    );
    assert_eq!(reopened.queued(), 1);
}

#[cfg(feature = "native-engine")]
#[test]
fn detached_preguest_boundary_change_refuses_without_guest_and_preserves_publication() {
    struct NoEntry;
    impl Engine for NoEntry {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
            panic!("changed boundary must refuse BEFORE guest entry")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = setup();
    let mut original = service(&path, &setup, 8);
    prepared(&mut original, &setup);
    original.debug_executing = true;
    let run = original
        .prepare_detached(&setup["inputs"])
        .unwrap()
        .unwrap();
    // Models a real separately staged terminal adapter inbox publication
    // between unlocked native admission and its preguest WAL fence.
    let mut changed = original.state.clone();
    RuntimeService::receiver_stage(&mut changed, &inbox(&setup, 90, 900)).unwrap();
    original.commit_receiver(changed).unwrap();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(original));
    let error =
        RuntimeService::execute_detached(&shared, run, &mut NoEntry, &|f| f(), &|_, _| Ok(()))
            .unwrap_err();
    assert_eq!(error, "detached attempt reservation changed");
    let mut locked = shared.lock().unwrap();
    assert!(locked.state.runtime.unresolved_attempt.is_none());
    assert!(locked.records().is_empty());
    assert!(
        locked.detached_execution,
        "caller retains engine/reservation custody until atomic restore"
    );
    assert_eq!(
        locked.state.staged_boundary.as_ref().unwrap().changes.len(),
        1
    );
    locked.detached_execution = false;
    locked.debug_executing = false;
    drop(locked);
    drop(shared);
    let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(
        crate::boundary::inbox_with_profile(
            &reopened.receiver_snapshot().files,
            reopened.receiver_snapshot().contract_profile,
        )
        .unwrap()
        .len(),
        1
    );
    assert_eq!(
        reopened.queued(),
        2,
        "old selected wake and separately committed new wake survive"
    );
}

#[cfg(feature = "native-engine")]
#[test]
fn detached_live_gate_refuses_before_fence_and_custody_is_released_only_by_caller() {
    struct NoEntry;
    impl Engine for NoEntry {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
            panic!("fresh authority refused");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let setup = setup();
    let mut original = service(&dir.path().join("state.wal"), &setup, 8);
    prepared(&mut original, &setup);
    original.debug_executing = true;
    let run = original
        .prepare_detached(&setup["inputs"])
        .unwrap()
        .unwrap();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(original));
    let called = std::cell::Cell::new(false);
    assert_eq!(
        RuntimeService::execute_detached(&shared, run, &mut NoEntry, &|f| f(), &|locked, _| {
            assert!(locked.detached_execution);
            called.set(true);
            Err("permission_denied".into())
        })
        .unwrap_err(),
        "permission_denied"
    );
    assert!(called.get());
    let mut locked = shared.lock().unwrap();
    assert!(locked.state.runtime.unresolved_attempt.is_none());
    assert!(locked.detached_execution);
    assert!(locked.receiver_pending().is_none());
    assert!(
        locked.prepare_detached(&setup["inputs"]).is_err(),
        "no second attempt before original caller's atomic restore"
    );
    assert!(locked.records().is_empty());
}

#[test]
fn prepared_step_revocation_and_cancel_retire_source_reservations_atomically() {
    for cancel in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.wal");
        let setup = setup();
        let mut original = service(&path, &setup, 8);
        let step = prepared(&mut original, &setup);
        drop(original);
        let mut reopened = RuntimeService::open(&path, &setup, 8).unwrap();
        let id = step["correlation_id"].as_str().unwrap();
        if cancel {
            reopened
                .receiver_cancel(id, step["context"]["actor"].as_str().unwrap(), None)
                .unwrap();
        } else {
            reopened
                .receiver_apply(id, &|| Err("permission_denied".into()))
                .unwrap();
        }
        assert!(reopened.state.receiver.active_step.is_none());
        assert_eq!(reopened.state.receiver.retired_wakes.len(), 1);
        assert_eq!(reopened.queued(), 0);
        drop(reopened);
        let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
        assert_eq!(reopened.receiver_pending_len(), 0);
        assert_eq!(reopened.state.runtime.next_source_seq, 1);
        assert_eq!(
            reopened.state.receiver.operations[id].receipt["state"],
            if cancel { "cancelled" } else { "failed" }
        );
    }
}

#[test]
fn unresolved_step_cannot_cancel_and_reconciliation_finishes_original_receipt() {
    struct Lost;
    impl Engine for Lost {
        fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
            Err("original worker observation withheld".into())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = setup();
    let mut original = service(&path, &setup, 8);
    let step = prepared(&mut original, &setup);
    let mut observer = original.state.runtime.clone();
    let event = original.state.router.events().next().unwrap().clone();
    let mut inputs = setup["inputs"].clone();
    inputs["events"] = json!([event]);
    original.debug_executing = true;
    assert!(original.run_next(&inputs, &mut Lost).is_err());
    original.debug_executing = false;
    let id = step["correlation_id"].as_str().unwrap();
    drop(original);
    let mut reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    let stop_id = "ab".repeat(16);
    let intent = "cd".repeat(32);
    let scope = crate::execution::managed::ReceiptScope {
        actor: step["context"]["actor"].as_str().unwrap().into(),
        ticket: None,
        method: "hub.stop".into(),
    };
    assert_eq!(
        reopened
            .managed_operation(&stop_id, &intent, false, 0, &scope)
            .unwrap()["state"],
        "queued"
    );
    drop(reopened);
    let mut reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert!(!reopened.managed_running());
    assert_eq!(
        reopened
            .managed_operation_receipt(&stop_id, &intent)
            .unwrap()
            .unwrap()["state"],
        "queued"
    );
    assert_eq!(
        reopened
            .receiver_cancel(id, step["context"]["actor"].as_str().unwrap(), None)
            .unwrap_err(),
        "conflict"
    );
    assert_eq!(
        reopened
            .receiver_apply(id, &|| Err("permission_denied".into()))
            .unwrap_err(),
        "unavailable"
    );
    let observed = observer
        .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
        .unwrap();
    reopened.recover_attempt(&observed["journal"]).unwrap();
    assert_eq!(
        reopened
            .managed_operation_receipt(&stop_id, &intent)
            .unwrap()
            .unwrap()["state"],
        "applied"
    );
    assert!(reopened.state.receiver.active_step.is_none());
    assert_eq!(reopened.receiver_pending_len(), 0);
    assert_eq!(reopened.receiver_admit(&step).unwrap()["state"], "applied");
    let response = reopened.receiver_step_result(id).unwrap();
    drop(reopened);
    let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(reopened.records().len(), 1);
    assert_eq!(reopened.receiver_step_result(id).unwrap(), response);
    assert_eq!(reopened.queued(), 0);
}
