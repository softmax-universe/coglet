use super::*;
use sha2::{Digest, Sha256};

fn cancel(db: &Database, attempt: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(b"factory-v1");
    for part in ["factory-cancel", KEY] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    let id: String = hash.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let payload = hex_profile(&[
        ("target_intent", KEY),
        ("target_task", TASK),
        ("attempt", &attempt.to_string()),
    ]);
    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute("INSERT INTO outbox(intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version) VALUES (?1,?1,'factory.cancel',?2,?1,'pending',0,1)", rusqlite::params![id, payload]).unwrap();
    id
}

#[test]
fn normal_cycle_cancels_before_submit_and_redelivers_after_reopen() {
    let (root, mut db) = seed();
    let cancellation = cancel(&db, 1);
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let batch = host.run(&mut db, 10).unwrap();
    assert!(host.dispatcher().adapter.created.is_empty());
    assert_eq!(batch.follow_ups.len(), 1);
    assert_eq!(batch.follow_ups[0].kind, "task.cancelled");
    assert_eq!(batch.follow_ups[0].lease, "");
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "cancelled");
    assert_eq!(
        db.get_outbox(&cancellation).unwrap().unwrap().state,
        "succeeded"
    );
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let replay = host.run(&mut db, 11).unwrap();
    assert_eq!(replay.follow_ups, batch.follow_ups);
    assert!(host.dispatcher().adapter.created.is_empty());
}

#[test]
fn normal_cycle_consumes_live_cancel_after_reopen() {
    let (root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.run(&mut db, 10).unwrap();
    cancel(&db, 1);
    let adapter = host.dispatcher().adapter.clone();
    drop(host);
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(adapter);
    let batch = host.run(&mut db, 11).unwrap();
    assert_eq!(host.dispatcher().adapter.cancels.len(), 1);
    assert_eq!(batch.follow_ups.len(), 1);
    assert_eq!(batch.follow_ups[0].kind, "task.cancelled");
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "done");
    host.run(&mut db, 12).unwrap();
    assert_eq!(host.dispatcher().adapter.cancels.len(), 1);
}

#[test]
fn delayed_cancellation_does_not_cancel_newer_lease() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.run(&mut db, 10).unwrap();
    host.run(&mut db, 70_000).unwrap();
    let current = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(current.attempt, 2);
    cancel(&db, 1);
    let batch = host.run(&mut db, 70_001).unwrap();
    assert!(host.dispatcher().adapter.cancels.is_empty());
    assert!(batch.follow_ups.is_empty());
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap(), current);
}

#[test]
fn cancellation_holds_uncertain_create_without_resubmit_or_false_result() {
    let (root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter {
        crash_keys: vec![KEY.into()],
        ..Default::default()
    });
    host.run(&mut db, 10).unwrap();
    cancel(&db, 1);
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let batch = host.run(&mut db, 11).unwrap();
    assert!(host.dispatcher().adapter.created.is_empty());
    assert!(batch.follow_ups.is_empty());
    host.run(&mut db, 12).unwrap();
    assert!(host.dispatcher().adapter.created.is_empty());
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "dispatching");
}

#[test]
fn unsupported_cancel_retains_http_result_correlation() {
    let (_root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 10).unwrap();
    cancel(&db, 1);
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    let batch = host.run(&mut db, 100_000).unwrap();
    assert!(batch.follow_ups.is_empty());
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap(), queue);
    assert_eq!(server.len(), 1);
    // Unsupported cancellation cannot stop another pending intent from being
    // dispatched, nor prevent either exact retained run's result being ingested.
    const OTHER: &str = "22222222222222222222222222222222";
    const OTHER_KEY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute(
        "INSERT INTO objects VALUES(?1,'task','dispatched','{}',1,1,1)",
        [OTHER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO outbox VALUES(?1,?1,'factory.dispatch',?2,?3,'pending',0,1)",
        rusqlite::params![OTHER_KEY, hex_profile(&[("task_id", OTHER)]), OTHER],
    )
    .unwrap();
    drop(conn);
    host.run(&mut db, 100_001).unwrap();
    assert_eq!(server.len(), 2);
    let remote = db.get_binding(KEY).unwrap().unwrap();
    let other_remote = db.get_binding(OTHER_KEY).unwrap().unwrap();
    server.post_result(&other_remote, true, "sibling complete");
    let batch = host.run(&mut db, 100_002).unwrap();
    assert!(
        batch
            .follow_ups
            .iter()
            .any(|f| f.task_id == OTHER && f.kind == "task.completed")
    );
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap(), queue);
    server.post_result(&remote, true, "retained attempt complete");
    let batch = host.run(&mut db, 100_003).unwrap();
    assert!(batch.follow_ups.iter().any(|f| f.task_id == TASK
        && f.kind == "task.completed"
        && f.attempt == queue.attempt
        && Some(&f.lease) == queue.lease_owner.as_ref()));
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "succeeded");
}

#[test]
fn cancellation_after_local_claim_before_submit_holds_without_create() {
    let (root, mut db) = seed();
    let claimed = db.claim_pending_outbox(1).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempt, 1);
    cancel(&db, 1);
    drop(db); // crash before submitting or saving any remote binding
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    for now in [10, 70_000] {
        let batch = host.run(&mut db, now).unwrap();
        assert!(batch.follow_ups.is_empty());
        assert!(host.dispatcher().adapter.created.is_empty());
        assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "dispatching");
    }
    // No persisted submit-start marker distinguishes this from a lost create
    // reply; retaining uncertainty is deliberate, not successful cancellation.
}

#[test]
fn direct_cancel_refuses_expired_and_terminal_attempts_before_io() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.run(&mut db, 10).unwrap();
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    let token = queue.lease_owner.as_deref().unwrap();
    assert!(
        host.dispatcher()
            .cancel_intent(
                &mut db,
                KEY,
                queue.attempt,
                token,
                queue.lease_until.unwrap()
            )
            .is_err()
    );
    assert!(host.dispatcher().adapter.cancels.is_empty());
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap(), queue);
    let done = report(
        &db,
        Outcome::Succeeded {
            note: "done".into(),
        },
    );
    host.dispatcher().apply_result(&mut db, KEY, done).unwrap();
    assert!(
        host.dispatcher()
            .cancel_intent(&mut db, KEY, queue.attempt, token, 11)
            .is_err()
    );
    assert!(host.dispatcher().adapter.cancels.is_empty());
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "succeeded");
}
