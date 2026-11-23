use super::*;
use std::time::{Duration, Instant};

#[path = "../../../coglets/factory/src/catalog.rs"]
mod factory_catalog;

fn factory_deployment_catalog() -> Catalog {
    let kind = |s| match s {
        "text" => CellType::Text,
        "int64" => CellType::Integer,
        "nullable_text" => CellType::NullableText,
        _ => panic!("unknown Factory catalog type"),
    };
    Catalog::approve_typed(
        factory_catalog::STATEMENTS
            .iter()
            .map(|s| {
                let statement = Statement::new(
                    s.id,
                    s.sql,
                    if s.mode == "read" {
                        Mode::Read
                    } else {
                        Mode::Write
                    },
                    s.parameters.len(),
                );
                TypedStatement {
                    statement: s.expected_affected.map_or(statement, |_| {
                        Statement::new(s.id, s.sql, Mode::Write, s.parameters.len())
                            .expect_affected(s.expected_affected.unwrap())
                    }),
                    parameters: s.parameters.iter().map(|t| kind(*t)).collect(),
                    columns: s
                        .columns
                        .iter()
                        .map(|(name, t)| Column {
                            name: (*name).into(),
                            kind: kind(*t),
                        })
                        .collect(),
                    tables: std::iter::once(s.table)
                        .chain(s.read_dependencies.iter().copied())
                        .map(str::to_string)
                        .collect(),
                    order: s.order.iter().map(|s| (*s).into()).collect(),
                }
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn factory_catalog_runs_against_its_actual_schema() {
    let catalog = factory_deployment_catalog();
    assert_eq!(catalog.digest().unwrap().len(), 64);
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "factory").unwrap();
    db.provision_for_test(include_str!("../../../coglets/factory/migrations/0001.sql"))
        .unwrap();
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    let params = [
        Cell::Text("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
        Cell::Text("task".into()),
        Cell::Text("open".into()),
        Cell::Text("{}".into()),
        Cell::Integer(1),
        Cell::Integer(10),
        Cell::Integer(10),
    ];
    assert_eq!(
        unit.statement("objects_create", Mode::Write, &params, 64, 65536)
            .unwrap()
            .affected,
        1
    );
    let page = unit
        .statement("objects_get", Mode::Read, &params[..1], 64, 65536)
        .unwrap();
    assert_eq!(page.rows[0], params);
    unit.statement(
        "events_append",
        Mode::Write,
        &[
            Cell::Text("01ARZ3NDEKTSV4RRFFQ69G5FAW".into()),
            params[0].clone(),
            Cell::Text("created".into()),
            Cell::Text("{}".into()),
            Cell::Integer(10),
            Cell::Integer(1),
        ],
        64,
        65536,
    )
    .unwrap();
    unit.commit_for_test().unwrap();
    assert!(db.connection.execute("INSERT INTO objects(id,type,status,data_json,version,created_at,updated_at) VALUES (NULL,'task','open','{}',1,0,0)", []).is_err());
}

#[test]
fn typed_catalog_refuses_sqlite_parameter_coercion() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "typed").unwrap();
    db.provision_for_test(
        "CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL, version INTEGER NOT NULL)",
    )
    .unwrap();
    let catalog = Catalog::approve_typed(vec![TypedStatement {
        statement: Statement::new(
            "put",
            "INSERT INTO objects(id, version) VALUES (?1, ?2)",
            Mode::Write,
            2,
        ),
        parameters: vec![CellType::Text, CellType::Integer],
        columns: vec![],
        tables: vec!["objects".into()],
        order: vec![],
    }])
    .unwrap();
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    let error = unit
        .statement(
            "put",
            Mode::Write,
            &[Cell::Text("a".into()), Cell::Text("123".into())],
            64,
            65536,
        )
        .unwrap_err();
    assert_eq!(error, "parameter type mismatch");
    assert!(unit.commit_for_test().is_err());
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM objects", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn typed_read(
    sql: &str,
    column: &str,
    kind: CellType,
    table: &str,
    order: &[&str],
) -> TypedStatement {
    TypedStatement {
        statement: Statement::new("read", sql, Mode::Read, 0),
        parameters: vec![],
        columns: vec![Column {
            name: column.into(),
            kind,
        }],
        tables: vec![table.into()],
        order: order.iter().map(|s| (*s).into()).collect(),
    }
}

#[test]
fn typed_catalog_enforces_tables_and_result_types() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "metadata").unwrap();
    db.provision_for_test("CREATE TABLE objects(id INTEGER PRIMARY KEY, value TEXT NOT NULL); INSERT INTO objects VALUES (1, 'text'); CREATE TABLE relations(id INTEGER PRIMARY KEY)").unwrap();
    for (sql, column, kind, table, reason) in [
        (
            "SELECT id FROM relations ORDER BY id",
            "id",
            CellType::Integer,
            "objects",
            "access to relations.id is prohibited",
        ),
        (
            "SELECT id FROM objects ORDER BY id",
            "wrong",
            CellType::Integer,
            "objects",
            "result column binding mismatch",
        ),
        (
            "SELECT value FROM objects ORDER BY id",
            "value",
            CellType::Integer,
            "objects",
            "result type mismatch",
        ),
    ] {
        let catalog =
            Catalog::approve_typed(vec![typed_read(sql, column, kind, table, &["id"])]).unwrap();
        let mut unit = db
            .begin(
                &catalog,
                Grants {
                    read: true,
                    write: false,
                },
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
        let error = unit
            .statement("read", Mode::Read, &[], 64, 65536)
            .unwrap_err();
        assert!(
            error.contains(reason),
            "{sql}: expected {reason}, got {error}"
        );
        assert!(unit.commit_for_test().is_err());
    }
}

#[test]
fn typed_catalog_requires_schema_unique_total_order() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "ordering").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL, value TEXT, nullable_unique TEXT UNIQUE); CREATE UNIQUE INDEX partial_key ON objects(value) WHERE value IS NOT NULL").unwrap();
    for (sql, order, valid) in [
        ("SELECT id FROM objects ORDER BY id", vec!["id"], true),
        (
            "SELECT id FROM objects ORDER BY value, id",
            vec!["value", "id"],
            true,
        ),
        (
            "SELECT id FROM objects ORDER BY value",
            vec!["value"],
            false,
        ),
        (
            "SELECT id FROM objects ORDER BY nullable_unique",
            vec!["nullable_unique"],
            false,
        ),
        ("SELECT id FROM objects ORDER BY id", vec!["value"], false),
        ("SELECT id FROM objects ORDER BY id DESC", vec!["id"], false),
    ] {
        let catalog = Catalog::approve_typed(vec![typed_read(
            sql,
            "id",
            CellType::Text,
            "objects",
            &order,
        )]);
        let result = catalog.and_then(|catalog| {
            db.begin(
                &catalog,
                Grants {
                    read: true,
                    write: false,
                },
                Instant::now() + Duration::from_secs(2),
            )?
            .statement("read", Mode::Read, &[], 64, 65536)
        });
        assert_eq!(result.is_ok(), valid, "{sql}: {result:?}");
    }
    // SQLite's INTEGER PRIMARY KEY DESC is NOT a rowid alias and allows NULL.
    db.provision_for_test("DROP TABLE objects; CREATE TABLE objects(id INTEGER PRIMARY KEY DESC)")
        .unwrap();
    let catalog = Catalog::approve_typed(vec![typed_read(
        "SELECT id FROM objects ORDER BY id",
        "id",
        CellType::Integer,
        "objects",
        &["id"],
    )])
    .unwrap();
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(unit.statement("read", Mode::Read, &[], 64, 65536).is_err());
}

#[test]
fn typed_catalog_rejects_self_join_and_subquery_order_spoofs() {
    for sql in [
        "SELECT a.id FROM objects a, objects b ORDER BY id",
        "SELECT id FROM objects WHERE id IN SELECT id FROM objects ORDER BY id",
        "SELECT id FROM objects CROSS JOIN objects b ORDER BY id",
    ] {
        assert!(
            Catalog::approve_typed(vec![typed_read(
                sql,
                "id",
                CellType::Text,
                "objects",
                &["id"]
            )])
            .is_err(),
            "{sql}"
        );
    }
}

#[test]
fn typed_catalog_digest_is_canonical_and_binds_all_metadata() {
    fn entry() -> TypedStatement {
        typed_read(
            "SELECT id FROM objects ORDER BY id",
            "id",
            CellType::Text,
            "objects",
            &["id"],
        )
    }
    let baseline = Catalog::approve_typed(vec![entry()])
        .unwrap()
        .digest()
        .unwrap();
    assert_eq!(baseline.len(), 64);
    assert_eq!(
        baseline,
        Catalog::approve_typed(vec![entry()])
            .unwrap()
            .digest()
            .unwrap()
    );
    for change in 0..6 {
        let mut item = entry();
        match change {
            0 => item.statement.id = "other".into(),
            1 => item.statement.sql = "SELECT id FROM objects WHERE id=?1 ORDER BY id".into(),
            2 => item.columns[0].kind = CellType::NullableText,
            3 => item.tables = vec!["relations".into()],
            4 => item.columns[0].name = "other".into(),
            _ => item.statement.expected_affected = Some(1),
        }
        if change == 1 {
            item.statement.parameters = 1;
            item.parameters = vec![CellType::Text];
        }
        assert_ne!(
            baseline,
            Catalog::approve_typed(vec![item])
                .unwrap()
                .digest()
                .unwrap(),
            "{change}"
        );
    }
    let mut second = entry();
    second.statement.id = "second".into();
    let a = Catalog::approve_typed(vec![entry(), second])
        .unwrap()
        .digest()
        .unwrap();
    let mut second = entry();
    second.statement.id = "second".into();
    let b = Catalog::approve_typed(vec![second, entry()])
        .unwrap()
        .digest()
        .unwrap();
    assert_eq!(a, b);
    assert!(
        catalog().digest().is_err(),
        "untyped test catalog cannot be pinned"
    );
}

#[test]
fn typed_catalog_refuses_collation_order_mismatch() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "collation").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT COLLATE NOCASE NOT NULL); CREATE UNIQUE INDEX binary_key ON objects(id COLLATE BINARY); INSERT INTO objects VALUES ('a'),('A')").unwrap();
    let catalog = Catalog::approve_typed(vec![typed_read(
        "SELECT id FROM objects ORDER BY id",
        "id",
        CellType::Text,
        "objects",
        &["id"],
    )])
    .unwrap();
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(unit.statement("read", Mode::Read, &[], 64, 65536).is_err());
}

#[test]
fn rejected_calls_are_counted_and_permission_precedes_poison() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "attempts").unwrap();
    let catalog = catalog();
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(unit.statement("list", Mode::Read, &[], 0, 256).is_err());
    assert_eq!(unit.calls, 1);
    assert_eq!(
        unit.statement("list", Mode::Read, &[], 64, 65536)
            .unwrap_err(),
        "permission denied"
    );
    assert_eq!(unit.calls, 2);
}

#[test]
fn outbox_duplicate_idempotency_key_rejected() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "outbox").unwrap();
    db.provision_for_test(include_str!("../../../coglets/factory/migrations/0001.sql"))
        .unwrap();
    let catalog = factory_deployment_catalog();
    let intent = |id: &str, key: &str, value: &str| Intent {
        intent_id: id.repeat(32),
        idempotency_key: key.repeat(32),
        kind: "work.start".into(),
        correlation_id: "c".repeat(32),
        payload: coglet_types::Value::Map(vec![(
            coglet_types::Value::Text("value".into()),
            coglet_types::Value::Text(value.into()),
        )]),
    };
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
        .with_intent_grants(["work.start".to_string()])
        .unwrap();
    let first = unit.enqueue(&intent("1", "a", "one")).unwrap();
    assert!(!first.duplicate);
    unit.enqueue(&intent("2", "b", "two")).unwrap();
    // Lookup key/content precedes proposed ID uniqueness, even when the proposal
    // already belongs to another key. Duplicate alone creates no new work.
    let duplicate = unit.enqueue(&intent("2", "a", "one")).unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.intent_id, first.intent_id);
    unit.commit_for_test().unwrap();
    drop(db);
    let mut db = Database::open(root.path(), "outbox").unwrap();
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
        .with_intent_grants(["work.start".to_string()])
        .unwrap();
    assert!(
        unit.enqueue(&intent("3", "a", "changed"))
            .unwrap_err()
            .contains("idempotency conflict")
    );
    assert!(unit.enqueue(&intent("4", "d", "four")).is_err());
    assert!(unit.commit_for_test().is_err());
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    // A new key cannot steal an existing ID. Failed later work rolls back the
    // successful provisional insert earlier in the same unit.
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
        .with_intent_grants(["work.start".to_string()])
        .unwrap();
    unit.enqueue(&intent("4", "d", "four")).unwrap();
    assert_eq!(
        unit.enqueue(&intent("1", "e", "new-key")).unwrap_err(),
        "intent ID conflict"
    );
    assert!(unit.commit_for_test().is_err());
    // Identical duplicates are still authorized before lookup.
    for (write, kinds) in [(false, vec!["work.start".to_string()]), (true, vec![])] {
        let mut unit = db
            .begin(
                &catalog,
                Grants { read: true, write },
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap()
            .with_intent_grants(kinds)
            .unwrap();
        assert_eq!(
            unit.enqueue(&intent("9", "a", "one")).unwrap_err(),
            "permission denied"
        );
        assert!(unit.commit_for_test().is_err());
    }
    let mut unit = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
        .with_intent_grants(["work.start".to_string()])
        .unwrap();
    let mut changed_correlation = intent("9", "a", "one");
    changed_correlation.correlation_id = "d".repeat(32);
    assert_eq!(
        unit.enqueue(&changed_correlation).unwrap_err(),
        "idempotency conflict"
    );
    assert!(unit.commit_for_test().is_err());
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

fn catalog() -> Catalog {
    Catalog::approve(vec![
        Statement::new(
            "put",
            "INSERT INTO objects(id, value) VALUES (?1, ?2)",
            Mode::Write,
            2,
        ),
        Statement::new(
            "list",
            "SELECT id, value FROM objects ORDER BY id",
            Mode::Read,
            0,
        ),
    ])
    .unwrap()
}

#[cfg(unix)]
#[test]
fn private_paths_reject_symlinks() {
    use std::os::unix::fs::symlink;
    for component in ["directory", "owner.lock"] {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let directory = root.path().join(hex::encode(Sha256::digest(b"symlink")));
        if component == "directory" {
            symlink(target.path(), &directory).unwrap();
        } else {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            std::fs::write(target.path().join("lock"), b"unchanged").unwrap();
            symlink(target.path().join("lock"), directory.join("owner.lock")).unwrap();
        }
        assert!(
            Database::open(root.path(), "symlink").is_err(),
            "{component}"
        );
    }
}

#[test]
fn empty_page_still_obeys_encoded_byte_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "empty").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT)")
        .unwrap();
    let long_name = "x".repeat(220);
    let catalog = Catalog::approve(vec![Statement::new(
        "wide",
        &format!("SELECT id AS {long_name} FROM objects ORDER BY id"),
        Mode::Read,
        0,
    )])
    .unwrap();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(txn.statement("wide", Mode::Read, &[], 64, 256).is_err());
}

#[test]
fn txn_rollback_on_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "rollback").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .unwrap();
    let catalog = catalog();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    let params = [Cell::Text("a".into()), Cell::Text("first".into())];
    txn.statement("put", Mode::Write, &params, 64, 65536)
        .unwrap();
    assert!(
        txn.statement("put", Mode::Write, &params, 64, 65536)
            .is_err()
    );
    assert!(txn.statement("list", Mode::Read, &[], 64, 65536).is_err());
    assert!(txn.commit_for_test().is_err());
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    // A new unit is clean and sees its own provisional writes; Drop aborts them.
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    txn.statement("put", Mode::Write, &params, 64, 65536)
        .unwrap();
    assert_eq!(
        txn.statement("list", Mode::Read, &[], 64, 65536)
            .unwrap()
            .rows
            .len(),
        1
    );
    drop(txn);
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn fk_integrity_enforced() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "foreignkeys").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY); CREATE TABLE relations(id TEXT PRIMARY KEY, source TEXT NOT NULL REFERENCES objects(id))").unwrap();
    let catalog = Catalog::approve(vec![Statement::new(
        "relation",
        "INSERT INTO relations(id, source) VALUES (?1, ?2)",
        Mode::Write,
        2,
    )])
    .unwrap();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    let result = txn.statement(
        "relation",
        Mode::Write,
        &[Cell::Text("r".into()), Cell::Text("absent".into())],
        64,
        65536,
    );
    assert!(
        result
            .unwrap_err()
            .contains("FOREIGN KEY constraint failed")
    );
    assert!(txn.commit_for_test().is_err());
}

#[test]
fn read_and_write_grants_are_independent() {
    for grants in [
        Grants {
            read: false,
            write: true,
        },
        Grants {
            read: true,
            write: false,
        },
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut db = Database::open(root.path(), "grants").unwrap();
        db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT)")
            .unwrap();
        let catalog = catalog();
        let mut txn = db
            .begin(&catalog, grants, Instant::now() + Duration::from_secs(2))
            .unwrap();
        let result = if grants.read {
            txn.statement(
                "put",
                Mode::Write,
                &[Cell::Text("x".into()), Cell::Text("x".into())],
                64,
                65536,
            )
        } else {
            txn.statement("list", Mode::Read, &[], 64, 65536)
        };
        assert!(result.unwrap_err().contains("permission denied"));
        assert!(txn.commit_for_test().is_err());
    }
}

#[test]
fn optimistic_version_conflict() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "versions").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT, version INTEGER NOT NULL); INSERT INTO objects VALUES ('a','old',1)").unwrap();
    let catalog = Catalog::approve(vec![
        Statement::new(
            "update",
            "UPDATE objects SET value=?1, version=version+1 WHERE id=?2 AND version=?3",
            Mode::Write,
            3,
        )
        .expect_affected(1),
    ])
    .unwrap();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: false,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(
        txn.statement(
            "update",
            Mode::Write,
            &[
                Cell::Text("new".into()),
                Cell::Text("a".into()),
                Cell::Integer(0)
            ],
            64,
            65536
        )
        .is_err()
    );
    assert!(txn.commit_for_test().is_err());
    assert_eq!(
        db.connection
            .query_row("SELECT version FROM objects", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn sqlite_work_is_cancelled_at_step_deadline() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "deadline").unwrap();
    db.provision_for_test("CREATE TABLE objects(id INTEGER PRIMARY KEY, value INTEGER)")
        .unwrap();
    for i in 0..50 {
        db.connection
            .execute("INSERT INTO objects VALUES (?1, ?1)", [i])
            .unwrap();
    }
    let catalog = Catalog::approve(vec![Statement::new("slow", "SELECT a.id FROM objects a CROSS JOIN objects b CROSS JOIN objects c CROSS JOIN objects d ORDER BY a.value, b.value, c.value, d.value", Mode::Read, 0)]).unwrap();
    let interrupt = db.connection.get_interrupt_handle();
    let (done, cancelled) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if cancelled.recv_timeout(Duration::from_secs(1)).is_err() {
            interrupt.interrupt();
        }
    });
    let start = Instant::now();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            start + Duration::from_millis(10),
        )
        .unwrap();
    let result = txn.statement("slow", Mode::Read, &[], 64, 65536);
    let elapsed = start.elapsed();
    let _ = done.send(());
    watchdog.join().unwrap();
    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_millis(500),
        "SQLite escaped deadline: {elapsed:?}"
    );
}

#[test]
fn catalog_refuses_unsafe_sql() {
    for sql in [
        "PRAGMA foreign_keys=OFF",
        "SELECT random()",
        "SELECT * FROM objects; DELETE FROM objects",
        "INSERT OR REPLACE INTO objects VALUES (?1, ?2)",
        "SELECT json('{}')",
        "ATTACH DATABASE ':memory:' AS x",
    ] {
        assert!(
            Catalog::approve(vec![Statement::new("unsafe", sql, Mode::Read, 0)]).is_err(),
            "{sql}"
        );
    }
}

#[test]
fn catalog_cannot_access_host_metadata() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "policy").unwrap();
    db.provision_for_test(
        "CREATE TABLE outbox(id TEXT); CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT)",
    )
    .unwrap();
    let catalog = Catalog::approve(vec![Statement::new(
        "forbidden",
        "SELECT id FROM outbox ORDER BY id",
        Mode::Read,
        0,
    )])
    .unwrap();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(
        txn.statement("forbidden", Mode::Read, &[], 64, 65536)
            .is_err()
    );
}

#[test]
fn whole_encoded_reply_and_utf8_cells_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "bytes").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .unwrap();
    db.connection
        .execute("INSERT INTO objects VALUES ('a', ?1)", ["é".repeat(128)])
        .unwrap();
    let catalog = catalog();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(txn.statement("list", Mode::Read, &[], 64, 256).is_err());
    drop(txn);
    db.connection
        .execute("UPDATE objects SET value = ?1", ["é".repeat(2049)])
        .unwrap();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    assert!(txn.statement("list", Mode::Read, &[], 64, 65536).is_err());
}

#[test]
fn query_results_bounded() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "bounded").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO objects VALUES ('a','one'),('b','two')").unwrap();
    let catalog = catalog();
    let mut txn = db
        .begin(
            &catalog,
            Grants {
                read: true,
                write: true,
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    // Until authenticated pagination is implemented, overflow fails closed rather
    // than silently dropping rows or returning an unbounded result.
    assert!(txn.statement("list", Mode::Read, &[], 1, 65536).is_err());
    assert!(txn.commit_for_test().is_err());
}

#[test]
fn db_isolation_between_coglets() {
    let root = tempfile::tempdir().unwrap();
    let mut a = Database::open(root.path(), "alice").unwrap();
    let mut b = Database::open(root.path(), "bob").unwrap();
    // Test-only trusted provisioning, not a guest capability or migration approval.
    for db in [&mut a, &mut b] {
        db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
    }
    let catalog = catalog();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut txn = a
        .begin(
            &catalog,
            Grants {
                read: true,
                write: true,
            },
            deadline,
        )
        .unwrap();
    txn.statement(
        "put",
        Mode::Write,
        &[Cell::Text("a".into()), Cell::Text("private".into())],
        64,
        65536,
    )
    .unwrap();
    txn.commit_for_test().unwrap();
    let mut txn = b
        .begin(
            &catalog,
            Grants {
                read: true,
                write: false,
            },
            deadline,
        )
        .unwrap();
    let page = txn.statement("list", Mode::Read, &[], 64, 65536).unwrap();
    assert!(page.rows.is_empty());
    drop(txn);
    assert_ne!(a.path(), b.path());
    // Ownership lease refuses a second concurrent authority for the same owner.
    assert!(Database::open(root.path(), "alice").is_err());
}
