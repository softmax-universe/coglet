use coglet_db::{Database, OutboxState};
use coglet_factory_host::{Dispatcher, HermesAdapter, MockAdapter, Outcome};
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

#[test]
fn dispatch_retry_same_idempotency_key() {
    let (_root, mut db) = seed();
    let mut dispatcher = Dispatcher::new(MockAdapter {
        crash_keys: vec![KEY.into()],
        ..MockAdapter::default()
    });
    let first = dispatcher.dispatch_pending(&mut db, 10, 8).unwrap();
    assert_eq!(first.claimed, 1);
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "uncertain");
    let second = dispatcher.dispatch_pending(&mut db, 11, 8).unwrap();
    assert_eq!(second.claimed, 1);
    let row = db.get_outbox(KEY).unwrap().unwrap();
    assert_eq!(row.state, "dispatching");
    assert_eq!(row.attempt, 2);
    assert_eq!(row.idempotency_key, KEY);
    assert_eq!(dispatcher.adapter.created.len(), 1);
    let remote = dispatcher.adapter.created[KEY].clone();
    dispatcher
        .adapter
        .claim(&remote.remote_id, "worker", 70_000)
        .unwrap();
    dispatcher
        .adapter
        .progress(&remote.remote_id, "running")
        .unwrap();
    let follow = dispatcher
        .apply_result(
            &mut db,
            KEY,
            Outcome::Succeeded {
                note: "diagnostic done".into(),
            },
        )
        .unwrap();
    assert_eq!(follow.kind, "task.completed");
    assert_eq!(follow.task_id, TASK);
    assert_eq!(
        db.get_outbox(KEY).unwrap().unwrap().state,
        OutboxState::Succeeded.as_str()
    );
}

#[test]
fn backlog_completes_after_restart() {
    let (_root, mut db) = seed();
    let mut dispatcher = Dispatcher::new(MockAdapter::default());
    dispatcher.dispatch_pending(&mut db, 10, 8).unwrap();
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().state, "dispatching");
    let recovered = dispatcher.recover_and_dispatch(&mut db, 12, 8).unwrap();
    assert_eq!(recovered.claimed, 1);
    assert_eq!(db.get_outbox(KEY).unwrap().unwrap().attempt, 2);
    let follow = dispatcher
        .apply_result(
            &mut db,
            KEY,
            Outcome::Succeeded {
                note: "after restart".into(),
            },
        )
        .unwrap();
    assert_eq!(follow.kind, "task.completed");
}

#[test]
fn lease_expiration_requeues() {
    let (_root, mut db) = seed();
    let mut dispatcher = Dispatcher::new(MockAdapter::default());
    dispatcher.dispatch_pending(&mut db, 10, 8).unwrap();
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    db.put_work_queue(&coglet_db::WorkQueueRow {
        state: "leased".into(),
        lease_until: Some(10),
        version: queue.version,
        ..queue
    })
    .unwrap();
    let ids = dispatcher.expire_leases(&mut db, 10).unwrap();
    assert_eq!(ids, vec![TASK.to_string()]);
    assert_eq!(db.get_work_queue(TASK).unwrap().unwrap().state, "pending");
}

#[test]
fn failed_experiment_result_event() {
    let (_root, mut db) = seed();
    let mut dispatcher = Dispatcher::new(MockAdapter::default());
    dispatcher.dispatch_pending(&mut db, 10, 8).unwrap();
    let follow = dispatcher
        .apply_result(
            &mut db,
            KEY,
            Outcome::Failed {
                note: "still failing".into(),
            },
        )
        .unwrap();
    assert_eq!(follow.kind, "task.failed");
    assert_eq!(follow.task_id, TASK);
}
