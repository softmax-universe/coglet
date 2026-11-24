use super::*;
#[path = "engine_bridge_tests.rs"]
mod engine_bridge_tests;
use coglet_types::Value as V;
use std::collections::BTreeSet;
pub(super) const CODE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) fn catalog() -> Catalog {
    Catalog::approve_typed(vec![
        TypedStatement {
            statement: Statement::new(
                "scan",
                "SELECT id,value FROM objects WHERE id>=?1 ORDER BY id",
                Mode::Read,
                1,
            ),
            parameters: vec![CellType::Text],
            columns: vec![
                Column {
                    name: "id".into(),
                    kind: CellType::Text,
                },
                Column {
                    name: "value".into(),
                    kind: CellType::Text,
                },
            ],
            tables: vec!["objects".into()],
            order: vec!["id".into()],
        },
        TypedStatement {
            statement: Statement::new(
                "put",
                "INSERT INTO objects(id,value) VALUES (?1,?2)",
                Mode::Write,
                2,
            ),
            parameters: vec![CellType::Text, CellType::Text],
            columns: vec![],
            tables: vec!["objects".into()],
            order: vec![],
        },
    ])
    .unwrap()
}
fn setup(root: &Path) -> Database {
    let mut db = Database::open(root, "factory").unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);INSERT INTO objects VALUES('a','first'),('b','second'),('c','third');PRAGMA user_version=1;").unwrap();
    db
}
pub(super) fn admission(db: &Database, cat: &Catalog) -> Admission {
    let (schema_version, schema_hash) = db.schema_pin().unwrap();
    Admission {
        pins: Pins {
            code_hash: CODE.into(),
            schema_version,
            schema_hash,
            catalog_hash: cat.digest().unwrap(),
            grants: Grants {
                read: true,
                write: true,
            },
            intent_kinds: BTreeSet::new(),
        },
        schema_min: 1,
        schema_max: 1,
        deadline: Instant::now() + Duration::from_secs(10),
    }
}
fn begin<'a>(db: &'a mut Database, cat: &'a Catalog) -> Unit<'a> {
    let a = admission(db, cat);
    db.admit(cat, CODE, a).unwrap()
}
fn params() -> Vec<Cell> {
    vec![Cell::Text("a".into())]
}
fn token(db: &mut Database, cat: &Catalog) -> String {
    let mut unit = begin(db, cat);
    let page = unit
        .statement("scan", Mode::Read, &params(), 1, 65536)
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    let token = page.next_cursor.unwrap();
    unit.commit_for_test().unwrap();
    token
}
#[test]
fn admission_pins_exact_code_schema_catalog_and_owned_grants() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    for which in 0..6 {
        let mut a = admission(&db, &cat);
        match which {
            0 => a.pins.code_hash = "b".repeat(64),
            1 => a.pins.schema_hash = "b".repeat(64),
            2 => a.pins.schema_version = 2,
            3 => a.pins.catalog_hash = "b".repeat(64),
            4 => a.schema_min = 2,
            _ => a.deadline = Instant::now(),
        };
        assert!(db.admit(&cat, CODE, a).is_err());
        assert_eq!(db.counters().unwrap(), (0, 0));
    }
    let mut a = admission(&db, &cat);
    a.pins.grants.write = false;
    let mut unit = db.admit(&cat, CODE, a).unwrap();
    assert!(!unit.transaction);
    assert_eq!(unit.pins().unwrap().code_hash, CODE);
    assert_eq!(
        unit.statement(
            "put",
            Mode::Write,
            &[Cell::Text("d".into()), Cell::Text("x".into())],
            1,
            256
        )
        .unwrap_err(),
        "permission denied"
    );
    drop(unit);
    assert_eq!(db.counters().unwrap(), (0, 1));
}
#[test]
fn admitted_aborts_are_durable_and_revision_only_advances_for_committed_writes() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    {
        let mut unit = begin(&mut db, &cat);
        unit.statement(
            "put",
            Mode::Write,
            &[Cell::Text("d".into()), Cell::Text("x".into())],
            1,
            256,
        )
        .unwrap();
    }
    assert_eq!(db.counters().unwrap(), (0, 1));
    drop(db);
    let mut db = Database::open(root.path(), "factory").unwrap();
    assert_eq!(db.counters().unwrap(), (0, 1));
    begin(&mut db, &cat).commit_for_test().unwrap();
    assert_eq!(db.counters().unwrap(), (0, 2));
    let mut unit = begin(&mut db, &cat);
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("d".into()), Cell::Text("x".into())],
        1,
        256,
    )
    .unwrap();
    unit.commit_for_test().unwrap();
    assert_eq!(db.counters().unwrap(), (1, 3));
    drop(db);
    let db = Database::open(root.path(), "factory").unwrap();
    assert_eq!(db.counters().unwrap(), (1, 3));
}
#[test]
fn cursor_continues_after_reopen_and_rejects_forgery_binding_owner_revision() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let t = token(&mut db, &cat);
    drop(db);
    let mut db = Database::open(root.path(), "factory").unwrap();
    let mut unit = begin(&mut db, &cat);
    let page = unit
        .statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
        .unwrap();
    assert_eq!(page.rows[0][0], Cell::Text("b".into()));
    assert!(page.next_cursor.is_some());
    unit.commit_for_test().unwrap();
    for (p, tok) in [
        (params(), "0".repeat(64)),
        (vec![Cell::Text("b".into())], t.clone()),
    ] {
        let mut unit = begin(&mut db, &cat);
        assert_eq!(
            unit.statement_page("scan", Mode::Read, &p, 1, 256, Some(&tok))
                .unwrap_err(),
            "cursor conflict"
        );
    }
    let mut other = Database::open(root.path(), "other").unwrap();
    other.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);PRAGMA user_version=1").unwrap();
    {
        let mut unit = begin(&mut other, &cat);
        assert_eq!(
            unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
                .unwrap_err(),
            "cursor conflict"
        );
    }
    let mut unit = begin(&mut db, &cat);
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("d".into()), Cell::Text("x".into())],
        1,
        256,
    )
    .unwrap();
    unit.commit_for_test().unwrap();
    let mut unit = begin(&mut db, &cat);
    assert_eq!(
        unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
            .unwrap_err(),
        "cursor conflict"
    );
}
#[test]
fn cursor_expires_at_256_further_attempts_including_aborts() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let t = token(&mut db, &cat);
    for _ in 0..254 {
        drop(begin(&mut db, &cat));
    }
    {
        let mut unit = begin(&mut db, &cat);
        unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
            .unwrap();
    }
    {
        let mut unit = begin(&mut db, &cat);
        assert_eq!(
            unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
                .unwrap_err(),
            "cursor conflict"
        );
    }
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM _coglet_cursors", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn cursor_mismatch_after_schema_or_catalog_change_and_last_key_corruption() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let t = token(&mut db, &cat);
    db.connection
        .execute("UPDATE objects SET value='changed' WHERE id='a'", [])
        .unwrap();
    {
        let mut unit = begin(&mut db, &cat);
        assert_eq!(
            unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
                .unwrap_err(),
            "cursor conflict"
        );
    }
    let mut changed_catalog = catalog();
    changed_catalog
        .statements
        .get_mut("put")
        .unwrap()
        .expected_affected = Some(1);
    {
        let mut unit = begin(&mut db, &changed_catalog);
        assert_eq!(
            unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
                .unwrap_err(),
            "cursor conflict"
        );
    }
    db.connection
        .execute("ALTER TABLE objects ADD COLUMN extra TEXT", [])
        .unwrap();
    {
        let mut unit = begin(&mut db, &cat);
        assert_eq!(
            unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
                .unwrap_err(),
            "cursor conflict"
        );
    }
}
#[test]
fn paging_and_writes_cannot_mix_and_whole_cursor_envelope_fits() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    db.connection
        .execute("UPDATE objects SET value=?1", ["x".repeat(70)])
        .unwrap();
    let mut unit = begin(&mut db, &cat);
    let page = unit
        .statement("scan", Mode::Read, &params(), 64, 256)
        .unwrap();
    assert!(!page.rows.is_empty());
    assert!(page.next_cursor.is_some());
    assert!(page.encode(0).unwrap().len() <= 256);
    assert_eq!(
        unit.statement(
            "put",
            Mode::Write,
            &[Cell::Text("d".into()), Cell::Text("x".into())],
            1,
            256
        )
        .unwrap_err(),
        "paging conflict"
    );
    drop(unit);
    let t = token(&mut db, &cat);
    let mut unit = begin(&mut db, &cat);
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("d".into()), Cell::Text("x".into())],
        1,
        256,
    )
    .unwrap();
    assert_eq!(
        unit.statement_page("scan", Mode::Read, &params(), 1, 256, Some(&t))
            .unwrap_err(),
        "paging conflict"
    );
}
#[test]
fn uint64_is_exact_ordered_and_survives_sqlite_reopen() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "factory").unwrap();
    db.provision_for_test(
        "CREATE TABLE objects(id BLOB PRIMARY KEY NOT NULL);PRAGMA user_version=1",
    )
    .unwrap();
    let cat = Catalog::approve_typed(vec![
        TypedStatement {
            statement: Statement::new("put", "INSERT INTO objects(id) VALUES (?1)", Mode::Write, 1),
            parameters: vec![CellType::Unsigned],
            columns: vec![],
            tables: vec!["objects".into()],
            order: vec![],
        },
        TypedStatement {
            statement: Statement::new("scan", "SELECT id FROM objects ORDER BY id", Mode::Read, 0),
            parameters: vec![],
            columns: vec![Column {
                name: "id".into(),
                kind: CellType::Unsigned,
            }],
            tables: vec!["objects".into()],
            order: vec!["id".into()],
        },
    ])
    .unwrap();
    let values = [0, i64::MAX as u64, (i64::MAX as u64) + 1, u64::MAX];
    let mut unit = begin(&mut db, &cat);
    for n in values {
        unit.statement("put", Mode::Write, &[Cell::Unsigned(n)], 64, 65536)
            .unwrap();
    }
    unit.commit_for_test().unwrap();
    drop(db);
    let mut db = Database::open(root.path(), "factory").unwrap();
    let mut unit = begin(&mut db, &cat);
    let page = unit.statement("scan", Mode::Read, &[], 64, 65536).unwrap();
    assert_eq!(page.rows, values.map(|n| vec![Cell::Unsigned(n)]));
    coglet_types::decode_profile(&page.encode(1).unwrap()).unwrap();
}
fn request(id: &str, mode: &str) -> Vec<u8> {
    let t = |s: &str| V::Text(s.into());
    coglet_types::encode_typed(
        "database::db-request",
        &V::Map(vec![
            (t("operation"), t("statement")),
            (t("statement"), t(id)),
            (t("mode"), t(mode)),
            (t("params"), V::Array(vec![t("a")])),
            (t("max_rows"), V::Integer(1.into())),
            (t("max_bytes"), V::Integer(256.into())),
        ]),
    )
    .unwrap()
}
fn wire(unit: &mut Unit<'_>, bytes: &[u8], budget: u64) -> (CallResult, Vec<u8>, u64) {
    let mut memory = vec![0x55; 140000];
    memory[..bytes.len()].copy_from_slice(bytes);
    let mut b = HostBudget { remaining: budget };
    let r = unit.call(&mut memory, 0, bytes.len() as i32, 70000, 65540, &mut b);
    (r, memory, b.remaining)
}
#[test]
fn abi_ranges_shape_capacity_and_budget_reserve_precede_sql() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let bytes = request("scan", "read");
    for which in 0..5 {
        let mut unit = begin(&mut db, &cat);
        let mut mem = vec![0x55; 140000];
        mem[..bytes.len()].copy_from_slice(&bytes);
        let before = mem.clone();
        let mut b = HostBudget {
            remaining: 1_000_000,
        };
        let r = match which {
            0 => unit.call(&mut mem, -1, 4, 70000, 65540, &mut b),
            1 => unit.call(&mut mem, 0, bytes.len() as i32, 0, 65540, &mut b),
            2 => unit.call(&mut mem, 0, bytes.len() as i32, 70000, 65539, &mut b),
            3 => {
                b.remaining = bytes.len() as u64 + 65539;
                unit.call(&mut mem, 0, bytes.len() as i32, 70000, 65540, &mut b)
            }
            _ => unit.call(&mut mem, 0, 0, 70000, 65540, &mut b),
        };
        assert!(matches!(
            r,
            CallResult::InvalidPointer
                | CallResult::Return {
                    result: 1 | 3,
                    response: None
                }
        ));
        assert_eq!(mem, before);
        assert!(!unit.transaction);
        assert_eq!(unit.calls, 1);
        assert!(unit.poisoned);
    }
    let mut unit = begin(&mut db, &cat);
    let (r, memory, left) = wire(&mut unit, &bytes, 1_000_000);
    let CallResult::Return {
        result: 0,
        response: Some(response),
    } = r
    else {
        panic!("{r:?}")
    };
    coglet_types::validate(
        "database::db-page",
        &coglet_types::decode_profile(&response).unwrap(),
    )
    .unwrap();
    assert_eq!(&memory[70004..70004 + response.len()], &response);
    assert_eq!(
        left,
        1_000_000 - bytes.len() as u64 - 4 - 2 * response.len() as u64
    );
}
#[test]
fn typed_failure_poison_grant_precedence_and_call_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    let (r, _, _) = wire(&mut unit, &request("unknown", "read"), 1_000_000);
    let CallResult::Return {
        result: 0,
        response: Some(response),
    } = r
    else {
        panic!("{r:?}")
    };
    let value = coglet_types::decode_profile(&response).unwrap();
    coglet_types::validate("database::db-failed", &value).unwrap();
    assert!(unit.poisoned);
    assert!(matches!(
        wire(&mut unit, &request("scan", "read"), 1_000_000).0,
        CallResult::Return {
            result: 4,
            response: None
        }
    ));
    drop(unit);
    let mut a = admission(&db, &cat);
    a.pins.grants.read = false;
    let mut unit = db.admit(&cat, CODE, a).unwrap();
    unit.poisoned = true;
    assert!(matches!(
        wire(&mut unit, &request("scan", "read"), 1_000_000).0,
        CallResult::Return {
            result: 2,
            response: None
        }
    ));
    drop(unit);
    let mut unit = begin(&mut db, &cat);
    for _ in 0..32 {
        assert!(matches!(
            wire(&mut unit, &request("scan", "read"), 1_000_000).0,
            CallResult::Return { result: 0, .. }
        ));
    }
    assert!(matches!(
        wire(&mut unit, &request("scan", "read"), 1_000_000).0,
        CallResult::Return {
            result: 3,
            response: None
        }
    ));
    assert_eq!(unit.calls, 33);
}

#[test]
fn expired_call_releases_lock_before_unit_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("d".into()), Cell::Text("x".into())],
        1,
        256,
    )
    .unwrap();
    assert!(!unit.db.connection.is_autocommit());
    unit.deadline = Instant::now();
    assert!(matches!(
        wire(&mut unit, &request("scan", "read"), 1_000_000).0,
        CallResult::Return {
            result: 3,
            response: None
        }
    ));
    assert!(unit.db.connection.is_autocommit());
    let other = Connection::open(unit.db.path()).unwrap();
    other.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
    assert_eq!(
        other
            .query_row("SELECT COUNT(*) FROM objects WHERE id='d'", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn wire_constraint_failure_is_typed_and_rolls_back_immediately() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    let mut value = coglet_types::decode_profile(&request("put", "write")).unwrap();
    let V::Map(fields) = &mut value else { panic!() };
    for (key, v) in fields {
        if key.as_text() == Some("params") {
            *v = V::Array(vec![V::Text("a".into()), V::Text("duplicate".into())]);
        }
    }
    let bytes = coglet_types::encode_profile(&value).unwrap();
    let CallResult::Return {
        result: 0,
        response: Some(response),
    } = wire(&mut unit, &bytes, 1_000_000).0
    else {
        panic!()
    };
    let v = coglet_types::decode_profile(&response).unwrap();
    coglet_types::validate("database::db-failed", &v).unwrap();
    assert!(format!("{v:?}").contains("invalid_argument"));
    assert!(unit.poisoned);
    assert!(unit.db.connection.is_autocommit());
}

#[test]
fn full_width_counters_reject_overflow_without_wrapping() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    db.connection
        .execute(
            "UPDATE _coglet_state SET revision=?1, attempts=?2",
            rusqlite::params![
                u64::MAX.to_be_bytes().to_vec(),
                (u64::MAX - 1).to_be_bytes().to_vec()
            ],
        )
        .unwrap();
    let mut unit = begin(&mut db, &cat);
    assert_eq!(unit.attempt(), u64::MAX);
    assert_eq!(unit.revision(), u64::MAX);
    assert_eq!(
        unit.statement(
            "put",
            Mode::Write,
            &[Cell::Text("d".into()), Cell::Text("x".into())],
            1,
            256
        )
        .unwrap_err(),
        "revision overflow"
    );
    drop(unit);
    let a = admission(&db, &cat);
    assert!(db.admit(&cat, CODE, a).is_err());
    assert_eq!(db.counters().unwrap(), (u64::MAX, u64::MAX));
}

#[test]
fn wire_cursor_conflict_precedes_fresh_call_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let mut unit = begin(&mut db, &cat);
    unit.calls = 32;
    let mut v = coglet_types::decode_profile(&request("scan", "read")).unwrap();
    let V::Map(fields) = &mut v else { panic!() };
    fields.push((V::Text("cursor".into()), V::Text("forged".into())));
    let bytes = coglet_types::encode_profile(&v).unwrap();
    let CallResult::Return {
        result: 0,
        response: Some(response),
    } = wire(&mut unit, &bytes, 1_000_000).0
    else {
        panic!()
    };
    assert!(format!("{:?}", coglet_types::decode_profile(&response).unwrap()).contains("conflict"));
    assert_eq!(unit.calls, 33);
    assert!(unit.poisoned);
}

#[test]
fn encoded_reply_sizing_matches_materialization_at_cbor_boundaries() {
    for n in [0, 23, 24, 255, 256, 65535, 65536, u32::MAX as u64, u64::MAX] {
        let page = Page {
            columns: vec![],
            rows: vec![],
            affected: 0,
            next_cursor: None,
        };
        assert_eq!(page.encoded_len(n), page.encode(n).unwrap().len());
    }
    for n in [0, 23, 24, 255, 256, 65535, 65536, u32::MAX as u64, u64::MAX] {
        let page = Page {
            columns: vec!["id".into()],
            rows: vec![
                vec![Cell::Unsigned(n)],
                vec![Cell::Integer(i64::MIN)],
                vec![Cell::Text("é".repeat(128))],
            ],
            affected: n,
            next_cursor: Some("x".repeat(64)),
        };
        assert_eq!(page.encoded_len(n), page.encode(n).unwrap().len());
    }
}
