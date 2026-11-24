use coglet_db::{Database, OutboxState};
use coglet_factory_host::{FactoryHost, MockAdapter, MockKanbanServer, Outcome, WorkerReport};
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
    let server = MockKanbanServer::spawn();
    let mut host = FactoryHost::connect(server.base());
    host.run(&mut db, 10).unwrap();
    assert_eq!(server.len(), 1);
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "leased");
    drop(db);
    drop(host);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::connect(server.base());
    let recovered = host.run(&mut db, 12).unwrap();
    assert_eq!(
        server.len(),
        1,
        "restart must reuse the public idempotency key"
    );
    assert_eq!(recovered.claimed, 0);
    server.post_result(&format!("k{KEY}"), true, "after restart");
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
fn http_kanban_adapter_exercises_public_api() {
    let (_root, mut db) = seed();
    let server = MockKanbanServer::spawn();
    let mut host = FactoryHost::connect(server.base());
    host.run(&mut db, 10).unwrap();
    assert_eq!(server.len(), 1);
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(queue.state, "leased");
    let remote = format!("k{KEY}");
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
        let server = MockKanbanServer::spawn();
        let mut host = FactoryHost::connect(server.base());
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
            .unwrap()
    };
    assert_eq!(cancelled.kind, "task.cancelled");
}
