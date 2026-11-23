use coglet_factory::{Object, Repository};
use coglet_guest_rust::database::DbHost;
use coglet_types::{DatabaseDbPage, DatabaseDbStatement, Value};

struct SqliteHost {
    db: rusqlite::Connection,
}
impl SqliteHost {
    fn new() -> Self {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        db.execute_batch(coglet_factory::SCHEMA_SQL).unwrap();
        Self { db }
    }
}
impl DbHost for SqliteHost {
    fn db(&mut self, input: &[u8], output: &mut [u8]) -> i32 {
        // Test-only real SQLite transport: not runtime admission/recovery qualification.
        let request: DatabaseDbStatement =
            coglet_types::decode_typed("database::db-statement", input).unwrap();
        let definition = coglet_factory::catalog::STATEMENTS
            .iter()
            .find(|s| s.id == request.statement)
            .unwrap();
        let params: Vec<rusqlite::types::Value> = request
            .params
            .into_iter()
            .map(|v| match v {
                Value::Null => rusqlite::types::Value::Null,
                Value::Text(s) => s.into(),
                Value::Integer(n) => i64::try_from(i128::from(n)).unwrap().into(),
                _ => panic!("invalid param"),
            })
            .collect();
        let mut stmt = self.db.prepare(definition.sql).unwrap();
        let page = if request.mode == "write" {
            let affected = stmt.execute(rusqlite::params_from_iter(params)).unwrap() as u64;
            if definition.expected_affected.is_some_and(|n| n != affected) {
                return 4;
            }
            DatabaseDbPage {
                status: "page".into(),
                columns: vec![],
                rows: vec![],
                affected,
                revision: 0,
                next_cursor: Value::Null,
            }
        } else {
            let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let mut cursor = stmt.query(rusqlite::params_from_iter(params)).unwrap();
            let mut rows = vec![];
            while let Some(row) = cursor.next().unwrap() {
                let mut cells = vec![];
                for i in 0..columns.len() {
                    cells.push(match row.get_ref(i).unwrap() {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => Value::Integer(n.into()),
                        rusqlite::types::ValueRef::Text(s) => {
                            Value::Text(std::str::from_utf8(s).unwrap().into())
                        }
                        _ => panic!("bad value"),
                    });
                }
                rows.push(cells);
            }
            DatabaseDbPage {
                status: "page".into(),
                columns,
                rows,
                affected: 0,
                revision: 0,
                next_cursor: Value::Null,
            }
        };
        let raw = page.to_cbor().unwrap();
        output[..4].copy_from_slice(&(raw.len() as u32).to_be_bytes());
        output[4..4 + raw.len()].copy_from_slice(&raw);
        0
    }
}
fn object(id: &str) -> Object {
    Object {
        id: id.into(),
        object_type: "goal".into(),
        status: "open".into(),
        data_json: "{\"name\":\"first\"}".into(),
        version: 1,
        created_at: 10,
        updated_at: 10,
    }
}
const ID: &str = "00000000000000000000000001";
#[test]
fn factory_schema_has_all_tables_indexes_and_foreign_keys() {
    let host = SqliteHost::new();
    let tables: Vec<String> = host
        .db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        tables,
        [
            "events",
            "objects",
            "outbox",
            "relations",
            "schema_migrations",
            "work_queue"
        ]
    );
    assert!(host.db.execute("INSERT INTO relations(id,source_id,target_id,type,data_json,version) VALUES ('r','absent','absent','depends_on','{}',1)", []).is_err());
    assert!(host.db.execute("INSERT INTO work_queue(task_id,state,priority,attempt,version) VALUES ('absent','pending',0,0,1)", []).is_err());
    assert!(host.db.execute("INSERT INTO events(id,object_id,event_type,payload_json,created_at,version) VALUES ('e','absent','created','{}',0,1)", []).is_err());
    host.db.execute("INSERT INTO outbox(intent_id,idempotency_key,kind,payload_json,correlation_id,state,attempt,version) VALUES ('a','key','test.run','{}','corr','pending',0,1)", []).unwrap();
    assert!(host.db.execute("INSERT INTO outbox(intent_id,idempotency_key,kind,payload_json,correlation_id,state,attempt,version) VALUES ('b','key','test.run','{}','corr','pending',0,1)", []).is_err());
}

#[test]
fn factory_updates_are_versioned_and_events_are_semantic_history() {
    let mut host = SqliteHost::new();
    let mut repo = Repository::new(&mut host);
    let mut goal = object(ID);
    repo.create_object(&goal).unwrap();
    goal.status = "done".into();
    repo.update_object(&goal).unwrap();
    assert_eq!(repo.get_object(ID).unwrap().unwrap().version, 2);
    assert!(repo.update_object(&goal).is_err());
    repo.append_event(&coglet_factory::SemanticEvent {
        id: "00000000000000000000000002".into(),
        object_id: Some(ID.into()),
        event_type: "goal.completed".into(),
        payload_json: "{}".into(),
        created_at: 20,
    })
    .unwrap();
    assert_eq!(
        host.db
            .query_row("SELECT event_type FROM events", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "goal.completed"
    );
}

#[test]
fn factory_queries_are_ordered_and_relation_rows_roundtrip() {
    let mut host = SqliteHost::new();
    let second = "00000000000000000000000002";
    {
        let mut repo = Repository::new(&mut host);
        repo.create_object(&object(second)).unwrap();
        repo.create_object(&object(ID)).unwrap();
        let page = repo.query_objects("goal", "open", 64, 65536, None).unwrap();
        assert_eq!(
            page.items.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            [ID, second]
        );
        assert!(page.next_cursor.is_none());
        assert!(
            repo.query_objects("task", "open", 64, 65536, None)
                .unwrap()
                .items
                .is_empty()
        );
    }
    host.db.execute("INSERT INTO relations(id,source_id,target_id,type,data_json,version) VALUES (?1,?2,?3,'depends_on','{}',1)", ["00000000000000000000000003",ID,second]).unwrap();
    let page = Repository::new(&mut host)
        .get_relations(ID, "depends_on", 64, 65536, None)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].target_id, second);
}

#[test]
fn application_ulids_use_only_supplied_recorded_inputs() {
    assert_eq!(coglet_factory::ulid(0, [0; 10]).unwrap(), "0".repeat(26));
    assert_eq!(
        coglet_factory::ulid((1u64 << 48) - 1, [255; 10]).unwrap(),
        format!("7{}", "Z".repeat(25))
    );
    assert!(coglet_factory::ulid(1u64 << 48, [0; 10]).is_err());
    assert!(
        coglet_factory::ulid(10, [1; 10]).unwrap() < coglet_factory::ulid(11, [0; 10]).unwrap()
    );
}

#[test]
fn repository_outbox_uses_dedicated_operation_not_sql() {
    struct EnqueueHost {
        calls: usize,
    }
    impl DbHost for EnqueueHost {
        fn db(&mut self, input: &[u8], output: &mut [u8]) -> i32 {
            let request: coglet_types::DatabaseDbEnqueue =
                coglet_types::decode_typed("database::db-enqueue", input).unwrap();
            self.calls += 1;
            assert_eq!(request.kind, "test.run");
            let reply = coglet_types::DatabaseDbEnqueued {
                status: "enqueued".into(),
                intent_id: request.intent_id,
                duplicate: false,
            }
            .to_cbor()
            .unwrap();
            output[..4].copy_from_slice(&(reply.len() as u32).to_be_bytes());
            output[4..4 + reply.len()].copy_from_slice(&reply);
            0
        }
    }
    let mut host = EnqueueHost { calls: 0 };
    let request = coglet_types::DatabaseDbEnqueue {
        operation: "enqueue".into(),
        intent_id: "1".repeat(32),
        idempotency_key: "2".repeat(32),
        kind: "test.run".into(),
        payload: Default::default(),
        correlation_id: "3".repeat(32),
    };
    assert_eq!(
        Repository::new(&mut host)
            .enqueue_outbox(&request)
            .unwrap()
            .intent_id,
        request.intent_id
    );
    assert_eq!(host.calls, 1);
}

#[test]
fn repository_refuses_zero_affected_optimistic_update_even_if_host_returns_page() {
    struct ZeroAffected;
    impl DbHost for ZeroAffected {
        fn db(&mut self, _: &[u8], output: &mut [u8]) -> i32 {
            let reply = DatabaseDbPage {
                status: "page".into(),
                columns: vec![],
                rows: vec![],
                affected: 0,
                revision: 0,
                next_cursor: Value::Null,
            }
            .to_cbor()
            .unwrap();
            output[..4].copy_from_slice(&(reply.len() as u32).to_be_bytes());
            output[4..4 + reply.len()].copy_from_slice(&reply);
            0
        }
    }
    assert!(
        Repository::new(&mut ZeroAffected)
            .update_object(&object(ID))
            .is_err()
    );
}

#[test]
fn factory_object_roundtrip_uses_sdk_buffer_and_portable_sql() {
    let mut host = SqliteHost::new();
    let mut repo = Repository::new(&mut host);
    assert!(repo.get_object(ID).unwrap().is_none());
    let expected = object(ID);
    repo.create_object(&expected).unwrap();
    assert_eq!(repo.get_object(ID).unwrap(), Some(expected));
}
