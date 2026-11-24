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
