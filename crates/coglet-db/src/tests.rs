use super::*;
use std::time::{Duration, Instant};

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
