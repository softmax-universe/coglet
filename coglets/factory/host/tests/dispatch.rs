use coglet_db::{Database, OutboxState};
use coglet_factory_host::{
    ContractKanbanServer, FactoryHost, HermesAdapter, MockAdapter, Outcome, WorkerReport,
};
use coglet_types::Value;

const OWNER: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TASK: &str = "11111111111111111111111111111111";
const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCHEMA: &str = include_str!("../../migrations/0001.sql");

fn seed() -> (tempfile::TempDir, Database) {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path(), OWNER).unwrap();
    let path = db.path().to_owned();
    drop(db);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute(
        "INSERT INTO objects(id,type,status,data_json,version,created_at,updated_at) VALUES (?1,'task','dispatched','{}',1,1,1)",
        [TASK],
    )
    .unwrap();
    let payload = hex_profile(&[("task_id", TASK)]);
    conn.execute(
        "INSERT INTO outbox(intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version) VALUES (?1,?1,'factory.dispatch',?2,?3,'pending',0,1)",
        rusqlite::params![KEY, payload, TASK],
    )
    .unwrap();
    drop(conn);
    let db = Database::open(root.path(), OWNER).unwrap();
    (root, db)
}

fn hex_profile(pairs: &[(&str, &str)]) -> String {
    let payload = Value::Map(
        pairs
            .iter()
            .map(|(k, v)| (Value::Text((*k).into()), Value::Text((*v).into())))
            .collect(),
    );
    coglet_types::encode_profile(&payload)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn report(db: &Database, outcome: Outcome) -> WorkerReport {
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    WorkerReport {
        attempt: queue.attempt,
        lease: queue.lease_owner.clone().unwrap(),
        outcome,
    }
}

fn event_count(db: &Database) -> i64 {
    let path = db.path().to_owned();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn dispatch_retry_same_idempotency_key() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter {
        crash_keys: vec![KEY.into()],
        ..MockAdapter::default()
    });
    let first = host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    assert_eq!(first.claimed, 1);
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "uncertain");
    let second = host.dispatcher().dispatch_pending(&mut db, 11, 8).unwrap();
    assert_eq!(second.claimed, 1);
    let row = db.get_outbox(KEY).unwrap().unwrap();
    assert_eq!(row.state, "dispatching");
    assert_eq!(row.attempt, 2);
    assert_eq!(row.idempotency_key, KEY);
    assert_eq!(host.dispatcher().adapter.created.len(), 1);
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(queue.state, "leased");
    host.dispatcher()
        .progress(
            &mut db,
            KEY,
            queue.attempt,
            queue.lease_owner.as_deref().unwrap(),
            "running",
        )
        .unwrap();
    let done = report(
        &db,
        Outcome::Succeeded {
            note: "diagnostic done".into(),
        },
    );
    let follow = host.dispatcher().apply_result(&mut db, KEY, done).unwrap();
    assert_eq!(follow.kind, "task.completed");
    assert_eq!(follow.task_id, TASK);
    assert_eq!(
        db.get_outbox(KEY).unwrap().unwrap().state,
        OutboxState::Succeeded.as_str()
    );
    assert_eq!(event_count(&db), 1);
}

#[test]
fn backlog_completes_after_restart() {
    let (root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 10).unwrap();
    assert_eq!(server.len(), 1);
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "leased");
    drop(db);
    drop(host);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(server.adapter());
    let recovered = host.run(&mut db, 12).unwrap();
    assert_eq!(
        server.len(),
        1,
        "restart must reuse the public idempotency key"
    );
    assert_eq!(recovered.claimed, 0);
    server.post_result(
        &db.get_binding(KEY).unwrap().unwrap(),
        true,
        "after restart",
    );
    let done = host.run(&mut db, 13).unwrap();
    assert_eq!(done.follow_ups[0].kind, "task.completed");
    assert_eq!(event_count(&db), 1);
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let again = host.run(&mut db, 14).unwrap();
    assert_eq!(again.follow_ups.len(), 1, "crash after commit redelivers");
    host.dispatcher()
        .acknowledge(&mut db, &again.follow_ups[0].event_id)
        .unwrap();
    assert!(host.run(&mut db, 15).unwrap().follow_ups.is_empty());
}

#[test]
fn lease_expiration_requeues() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    let leased = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(leased.state, "leased");
    assert_eq!(leased.lease_until, Some(60_010));
    assert!(!host.dispatcher().adapter.claims.is_empty());
    let ids = host.dispatcher().expire_leases(&mut db, 60_011).unwrap();
    assert_eq!(ids, vec![TASK.to_string()]);
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "pending");
    assert_eq!(
        db.get_work_queue(TASK).unwrap().unwrap().attempt,
        leased.attempt + 1
    );
    host.dispatcher()
        .redispatch_pending(&mut db, 60_012, 8)
        .unwrap();
}

#[test]
fn failed_experiment_result_event() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    let failed = report(
        &db,
        Outcome::Failed {
            note: "still failing".into(),
        },
    );
    let follow = host
        .dispatcher()
        .apply_result(&mut db, KEY, failed)
        .unwrap();
    assert_eq!(follow.kind, "task.failed");
    assert_eq!(follow.task_id, TASK);
    assert_eq!(event_count(&db), 1);
}

#[test]
fn crash_after_terminal_redelivers_until_ack() {
    let (root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    let succeeded = report(
        &db,
        Outcome::Succeeded {
            note: "unique evidence".into(),
        },
    );
    let follow = host
        .dispatcher()
        .apply_result(&mut db, KEY, succeeded)
        .unwrap();
    drop(follow);
    let path = db.path().to_owned();
    drop(db);
    drop(host);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(conn);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let recovered = host
        .dispatcher()
        .recover_and_dispatch(&mut db, 20, 8)
        .unwrap();
    assert_eq!(recovered.follow_ups.len(), 1);
    assert_eq!(recovered.follow_ups[0].note, "unique evidence");
    let replay = host
        .dispatcher()
        .apply_result(
            &mut db,
            KEY,
            WorkerReport {
                attempt: recovered.follow_ups[0].attempt,
                lease: recovered.follow_ups[0].lease.clone(),
                outcome: Outcome::Succeeded {
                    note: "unique evidence".into(),
                },
            },
        )
        .unwrap();
    assert_eq!(replay.event_id, recovered.follow_ups[0].event_id);
    assert_eq!(event_count(&db), 1);
}

#[test]
fn stale_attempt_does_not_finish_current_lease() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    let old = report(
        &db,
        Outcome::Succeeded {
            note: "late".into(),
        },
    );
    host.dispatcher().expire_leases(&mut db, 60_011).unwrap();
    host.dispatcher()
        .redispatch_pending(&mut db, 60_012, 8)
        .unwrap();
    let current = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(current.state, "leased");
    assert_ne!(current.lease_owner.as_deref(), Some(old.lease.as_str()));
    assert!(host.dispatcher().apply_result(&mut db, KEY, old).is_err());
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "dispatching");
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "leased");
    assert_eq!(event_count(&db), 0);
}

#[test]
fn hermes_contract_retry_cancel_progress() {
    let (_root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 10).unwrap();
    assert_eq!(server.len(), 1);
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(queue.state, "leased");
    let remote = db.get_binding(KEY).unwrap().unwrap();
    host.dispatcher()
        .heartbeat(
            &mut db,
            KEY,
            queue.attempt,
            queue.lease_owner.as_deref().unwrap(),
            20,
        )
        .unwrap();
    host.dispatcher()
        .progress(
            &mut db,
            KEY,
            queue.attempt,
            queue.lease_owner.as_deref().unwrap(),
            "building",
        )
        .unwrap();
    server.post_result(&remote, false, "trace mismatch");
    let ingested = host.run(&mut db, 30).unwrap();
    assert_eq!(ingested.follow_ups[0].kind, "task.failed");
    assert_eq!(ingested.follow_ups[0].note, "trace mismatch");
    assert_eq!(event_count(&db), 1);
    host.dispatcher().expire_leases(&mut db, 90_000).unwrap();
    let cancelled = {
        let (_root2, mut fresh) = seed();
        let server = ContractKanbanServer::spawn();
        let mut host = FactoryHost::from_adapter(server.adapter());
        host.run(&mut fresh, 10).unwrap();
        let queue = fresh.get_work_queue(TASK).unwrap().unwrap();
        host.dispatcher()
            .cancel_intent(
                &mut fresh,
                KEY,
                queue.attempt,
                queue.lease_owner.as_deref().unwrap(),
                40,
            )
            .unwrap_err()
    };
    assert!(cancelled.contains("unsupported"));
    let before = server.requests().len();
    assert!(host.dispatcher().adapter.retry(&remote).is_err());
    assert_eq!(
        server.requests().len(),
        before,
        "unsupported retry does no I/O"
    );
}

fn spec() -> coglet_factory_host::TaskSpec {
    coglet_factory_host::TaskSpec {
        idempotency_key: KEY.into(),
        kind: "factory.dispatch".into(),
        payload_cbor_hex: hex_profile(&[("task_id", TASK)]),
        correlation_id: TASK.into(),
        task_id: TASK.into(),
    }
}

#[test]
fn crash_after_binding_is_reconciled() {
    let (root, mut db) = seed();
    assert_eq!(db.claim_pending_outbox(1).unwrap().len(), 1);
    db.put_binding(KEY, &format!("k{KEY}"), TASK).unwrap();
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let recovered = host
        .dispatcher()
        .recover_and_dispatch(&mut db, 100, 8)
        .unwrap();
    assert_eq!(recovered.claimed, 1);
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "leased");
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "dispatching");
    assert_eq!(host.dispatcher().adapter.claims.len(), 1);
}

#[test]
fn crash_after_remote_claim_keeps_token_without_reclaim() {
    let (root, mut db) = seed();
    let rows = db.claim_pending_outbox(1).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let remote = host.dispatcher().adapter.create(&spec()).unwrap();
    db.put_binding(KEY, &remote.remote_id, TASK).unwrap();
    let lease = host
        .dispatcher()
        .adapter
        .claim(&remote.remote_id, "factory-host", 160_000, rows[0].attempt)
        .unwrap();
    db.set_binding_phase(KEY, "claimed", &lease.token).unwrap();
    let token = lease.token.clone();
    drop(db);
    drop(host);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher()
        .recover_and_dispatch(&mut db, 100, 8)
        .unwrap();
    assert_eq!(
        db.get_work_queue(TASK)
            .unwrap()
            .unwrap()
            .lease_owner
            .as_deref(),
        Some(token.as_str())
    );
    assert!(
        host.dispatcher().adapter.claims.is_empty(),
        "stored claim token must not be claimed again"
    );
}

#[test]
fn crash_during_uncertain_claim_is_reconciled() {
    let (root, mut db) = seed();
    db.claim_pending_outbox(1).unwrap();
    db.put_binding(KEY, &format!("k{KEY}"), TASK).unwrap();
    db.set_binding_phase(KEY, "claiming", "").unwrap();
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let recovered = host
        .dispatcher()
        .recover_and_dispatch(&mut db, 100, 8)
        .unwrap();
    assert_eq!(recovered.claimed, 1);
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "leased");
    assert_eq!(
        db.get_binding_record(KEY).unwrap().unwrap().phase,
        "claimed"
    );
}

#[test]
fn quoted_multiline_note_survives_redelivery() {
    let (root, mut db) = seed();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    let note = "trace says \"failed\"\nline two\\path \u{1F9EA}  spaced";
    let done = report(&db, Outcome::Succeeded { note: note.into() });
    let initial = host.dispatcher().apply_result(&mut db, KEY, done).unwrap();
    assert_eq!(initial.note, note);
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(queue.state, "done");
    assert_eq!(queue.lease_owner.as_deref(), Some(initial.lease.as_str()));
    drop(db);
    drop(host);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let replay = host.run(&mut db, 11).unwrap();
    assert_eq!(replay.follow_ups[0].note, note);
    let pretty = serde_json::to_string_pretty(&serde_json::json!({
        "task_id": TASK,
        "note": note,
        "attempt": replay.follow_ups[0].attempt,
        "lease": replay.follow_ups[0].lease,
        "kind": "task.completed",
    }))
    .unwrap();
    let path = db.path().to_owned();
    drop(db);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE objects SET data_json=?1 WHERE type='delivery' AND status='pending'",
        [pretty],
    )
    .unwrap();
    drop(conn);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let pretty_replay = host.run(&mut db, 12).unwrap();
    assert_eq!(pretty_replay.follow_ups[0].note, note);
}

#[test]
fn hermes_contract_fixture_create_claim_complete() {
    let server = ContractKanbanServer::spawn();
    let mut adapter = server.adapter();
    let task = adapter.create(&spec()).unwrap();
    assert_eq!(adapter.create(&spec()).unwrap(), task);
    assert_eq!(server.len(), 1);
    let lease = adapter
        .claim(&task.remote_id, "factory-host", 60_010, 1)
        .unwrap();
    assert!(
        adapter
            .poll(&task.remote_id, 1, &lease.token)
            .unwrap()
            .is_none()
    );
    server.post_result(&task.remote_id, true, "actual recorded Board schema");
    drop(adapter);
    let mut adapter = server.adapter();
    let report = adapter
        .poll(&task.remote_id, 1, &lease.token)
        .unwrap()
        .unwrap();
    assert_eq!(report.lease, lease.token);
    assert_eq!(
        report.outcome,
        Outcome::Succeeded {
            note: "actual recorded Board schema".into()
        }
    );
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r["path"].as_str().unwrap().starts_with("/v2/boards/"))
    );
    let mut changed = spec();
    changed.payload_cbor_hex = "00".into();
    assert!(adapter.create(&changed).is_err());
    assert!(adapter.poll(&task.remote_id, 2, &lease.token).is_err());
}

#[test]
fn unconfigured_transport_does_not_fabricate_worker_failure() {
    let (_root, mut db) = seed();
    let mut host = FactoryHost::connect("http://127.0.0.1:1");
    assert!(host.run(&mut db, 10).is_err());
    assert_ne!(db.get_outbox(KEY).unwrap().unwrap().state, "failed");
    assert!(db.pending_deliveries().unwrap().is_empty());
    let server = ContractKanbanServer::spawn();
    let adapter = coglet_factory_host::HttpKanban::authenticated(
        server.base(),
        "coglet-factory-t01a20f44e61e630a",
        "wrong-token",
        "worker-a1",
        "fixture",
    )
    .unwrap();
    let mut host = FactoryHost::from_adapter(adapter);
    host.dispatcher()
        .recover_and_dispatch(&mut db, 11, 8)
        .unwrap();
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "uncertain");
    assert!(db.pending_deliveries().unwrap().is_empty());
    assert!(server.is_empty());
}

#[test]
fn unsupported_retry_holds_expired_attempt_without_poisoning_cycle() {
    let (_root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 10).unwrap();
    let original = db.get_work_queue(TASK).unwrap().unwrap();
    let before = server.requests().len();
    let batch = host.run(&mut db, 90_000).unwrap();
    assert!(batch.expired.is_empty());
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap(), original);
    assert!(
        server.requests()[before..]
            .iter()
            .all(|r| r["method"] == "GET")
    );
    server.post_result(&db.get_binding(KEY).unwrap().unwrap(), true, "late result");
    assert_eq!(host.run(&mut db, 90_001).unwrap().follow_ups.len(), 1);
}

#[test]
fn factory_dispatch_crash_reconciles_exactly_once() {
    let (root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    server.lose_next_create_reply();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 10).unwrap();
    assert_eq!(server.len(), 1, "remote effect committed before lost reply");
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "uncertain");
    drop((host, db));
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.dispatcher()
        .recover_and_dispatch(&mut db, 11, 8)
        .unwrap();
    assert_eq!(server.len(), 1);
    server.post_result(
        &db.get_binding(KEY).unwrap().unwrap(),
        true,
        "persisted once",
    );
    let batch = host.run(&mut db, 12).unwrap();
    assert_eq!(batch.follow_ups.len(), 1);
    drop((host, db));
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(server.adapter());
    let replay = host.run(&mut db, 13).unwrap();
    assert_eq!(replay.follow_ups, batch.follow_ups);
    assert_eq!(event_count(&db), 1);
    host.dispatcher()
        .acknowledge(&mut db, &replay.follow_ups[0].event_id)
        .unwrap();
    assert!(host.run(&mut db, 14).unwrap().follow_ups.is_empty());
}

#[test]
fn lost_grant_reply_then_remote_completion_recovers() {
    for terminal in [false, true] {
        for expired in [false, true] {
            let (root, mut db) = seed();
            let server = ContractKanbanServer::spawn();
            server.lose_next_grant_reply();
            let mut host = FactoryHost::from_adapter(server.adapter());
            host.run(&mut db, 10).unwrap();
            let remote = db.get_binding(KEY).unwrap().unwrap();
            assert_eq!(
                db.get_binding_record(KEY).unwrap().unwrap().phase,
                "claiming"
            );
            assert!(db.get_work_queue(TASK).unwrap().is_none());
            if expired {
                server.expire_lease(&remote);
            }
            if terminal {
                server.post_result(&remote, true, "lost grant reply result");
            }
            drop((host, db));
            let mut db = Database::open(root.path(), OWNER).unwrap();
            let mut host = FactoryHost::from_adapter(server.adapter());
            let before = server.requests().len();
            let batch = host.run(&mut db, 20).unwrap();
            assert_eq!(batch.follow_ups.len(), usize::from(terminal));
            let queue = db.get_work_queue(TASK).unwrap().unwrap();
            assert_eq!(queue.attempt, 1);
            if expired && !terminal {
                assert_eq!(queue.lease_until, Some(0));
                let token = queue.lease_owner.as_deref().unwrap();
                let requests = server.requests().len();
                assert!(
                    host.dispatcher()
                        .heartbeat(&mut db, KEY, 1, token, 21)
                        .is_err()
                );
                assert!(
                    host.dispatcher()
                        .progress(&mut db, KEY, 1, token, "not authorized")
                        .is_err()
                );
                assert!(
                    host.dispatcher()
                        .cancel_intent(&mut db, KEY, 1, token, 21)
                        .is_err()
                );
                assert_eq!(server.requests().len(), requests);
            }
            assert!(
                server.requests()[before..]
                    .iter()
                    .all(|r| r["method"] == "GET")
            );
            if !terminal {
                server.post_result(&remote, true, "lost grant reply result");
            }
            let done = host.run(&mut db, 90_000).unwrap();
            assert_eq!(done.follow_ups.len(), 1);
            assert_eq!(event_count(&db), 1);
            drop((host, db));
            let mut db = Database::open(root.path(), OWNER).unwrap();
            let mut host = FactoryHost::from_adapter(server.adapter());
            assert_eq!(
                host.run(&mut db, 90_001).unwrap().follow_ups,
                done.follow_ups
            );
            assert_eq!(event_count(&db), 1);
            host.dispatcher()
                .acknowledge(&mut db, &done.follow_ups[0].event_id)
                .unwrap();
            assert!(host.run(&mut db, 90_002).unwrap().follow_ups.is_empty());
            assert_eq!(
                server
                    .requests()
                    .iter()
                    .filter(|r| r["body"]["action"] == "grant")
                    .count(),
                1
            );
        }
    }
}

#[test]
fn uncertain_grant_unknown_evidence_is_not_absence() {
    let (_root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    server.lose_next_grant_reply();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 10).unwrap();
    server.unknown_run_evidence(true);
    let before = server.requests().len();
    for now in [20, 90_000] {
        assert!(host.run(&mut db, now).unwrap().follow_ups.is_empty());
        assert!(db.get_work_queue(TASK).unwrap().is_none());
    }
    assert!(
        server.requests()[before..]
            .iter()
            .all(|r| r["method"] == "GET")
    );
    server.unknown_run_evidence(false);
    let remote = db.get_binding(KEY).unwrap().unwrap();
    server.change_run_holder(&remote);
    assert!(host.run(&mut db, 90_001).is_err());
    assert!(db.get_work_queue(TASK).unwrap().is_none());
    assert_eq!(event_count(&db), 0);
}

#[test]
fn uncertain_grant_proven_absent_can_claim() {
    let (_root, mut db) = seed();
    db.claim_pending_outbox(1).unwrap();
    let server = ContractKanbanServer::spawn();
    let remote = server.adapter().create(&spec()).unwrap();
    db.put_binding(KEY, &remote.remote_id, TASK).unwrap();
    db.set_binding_phase(KEY, "claiming", "").unwrap();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.run(&mut db, 20).unwrap();
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().attempt, 1);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r["body"]["action"] == "grant")
            .count(),
        1
    );
}

#[test]
fn contract_cached_grant_returns_current_lease() {
    let server = ContractKanbanServer::spawn();
    let mut adapter = server.adapter();
    let task = adapter.create(&spec()).unwrap();
    let original = adapter
        .claim(&task.remote_id, "factory-host", 60_010, 1)
        .unwrap();
    assert_eq!(
        adapter
            .claim(&task.remote_id, "factory-host", 60_010, 1)
            .unwrap()
            .token,
        original.token
    );
    server.expire_lease(&task.remote_id);
    assert_eq!(
        adapter.claim(&task.remote_id, "factory-host", 60_010, 1),
        Err(coglet_factory_host::AdapterError::Uncertain)
    );
    server.post_result(&task.remote_id, true, "done");
    assert_eq!(
        adapter.claim(&task.remote_id, "factory-host", 60_010, 1),
        Err(coglet_factory_host::AdapterError::Uncertain)
    );
    assert!(
        adapter
            .poll(&task.remote_id, 1, &original.token)
            .unwrap()
            .is_some()
    );
}

#[test]
fn remote_completion_before_local_lease_recovers_without_reclaim() {
    let (root, mut db) = seed();
    let server = ContractKanbanServer::spawn();
    let rows = db.claim_pending_outbox(1).unwrap();
    let mut adapter = server.adapter();
    let remote = adapter.create(&spec()).unwrap();
    db.put_binding(KEY, &remote.remote_id, TASK).unwrap();
    let lease = adapter
        .claim(&remote.remote_id, "factory-host", 60_010, rows[0].attempt)
        .unwrap();
    db.set_binding_phase(KEY, "claimed", &lease.token).unwrap();
    server.post_result(&remote.remote_id, true, "completed during crash");
    drop((adapter, db));
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(server.adapter());
    let before = server.requests().len();
    let batch = host
        .dispatcher()
        .recover_and_dispatch(&mut db, 20, 8)
        .unwrap();
    assert_eq!(batch.follow_ups.len(), 1);
    assert_eq!(batch.follow_ups[0].note, "completed during crash");
    assert_eq!(event_count(&db), 1);
    assert!(
        server.requests()[before..]
            .iter()
            .all(|r| r["method"] == "GET"),
        "terminal run is never renewed or reclaimed"
    );
}
