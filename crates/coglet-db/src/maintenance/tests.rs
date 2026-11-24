use super::*;

pub(super) const OWNER: &str =
    "coglet:1111111111111111111111111111111111111111111111111111111111111111";
const SQL: &str = "CREATE TABLE notes(id TEXT PRIMARY KEY NOT NULL, body TEXT NOT NULL);";
pub(super) const HISTORY: &str = "CREATE TABLE IF NOT EXISTS schema_migrations(version BIGINT PRIMARY KEY NOT NULL, applied_at BIGINT NOT NULL, migration_hash TEXT NOT NULL);";
struct Approval {
    digest: String,
}
impl OperatorApproval for Approval {
    fn verify_migration(&mut self, _: &Migration, digest: &str, proof: &[u8]) -> Result<()> {
        if digest == self.digest && proof == b"authenticated-operator-migration-approval" {
            Ok(())
        } else {
            Err("operator approval denied".into())
        }
    }
}
pub(super) struct Replay {
    head: String,
    fail: bool,
    calls: usize,
}
impl RestoreVerifier for Replay {
    fn verify_replay_and_references(&mut self, owner: &str, b: &RecoveryBundle) -> Result<()> {
        self.calls += 1;
        if self.fail || owner != OWNER || b.journal_head != self.head {
            return Err("replay/head/reference verification failed".into());
        }
        // This fixture has no guest steps/blobs; later tests add marker records.
        Ok(())
    }
}
pub(super) fn bundle() -> RecoveryBundle {
    RecoveryBundle {
        journal_head: "a".repeat(64),
        catalog_hash: "e".repeat(64),
        decisions: vec![],
        blobs: BTreeMap::new(),
    }
}
pub(super) fn replay() -> Replay {
    Replay {
        head: "a".repeat(64),
        fail: false,
        calls: 0,
    }
}
pub(super) fn provision(root: &Path) -> Database {
    let mut d = Database::open(root, OWNER).unwrap();
    d.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL); INSERT INTO objects VALUES('one','wal-data'); PRAGMA user_version=1; PRAGMA wal_autocheckpoint=0;").unwrap();
    d
}
pub(super) fn plan(db: &Database) -> Migration {
    let (from_version, from_schema_hash) = db.schema_pin().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut future = provision(root.path());
    future.provision_for_test(SQL).unwrap();
    future.provision_for_test(HISTORY).unwrap();
    Migration { owner: OWNER.into(), from_version, from_schema_hash, to_version: 2, to_schema_hash: future.schema_pin().unwrap().1, sql: SQL.into(), compatibility: CompatibilityFloor { previous: CodeRange { code_hash: "b".repeat(64), min: 1, max: 2 }, current: CodeRange { code_hash: "c".repeat(64), min: 1, max: 2 } }, recovery_plan: "Retain known-good snapshot; stop dispatch and repair in isolation; never rewind external effects".into() }
}
pub(super) fn apply(db: &mut Database, p: &Migration, parent: &Path) -> Result<Snapshot> {
    db.migrate(
        p,
        b"authenticated-operator-migration-approval",
        &mut Approval {
            digest: p.digest()?,
        },
        &parent.join("backup"),
        &parent.join("trial"),
        &bundle(),
        &mut replay(),
    )
}
#[test]
fn wal_safe_snapshot_restore_and_reopen_remain_isolated() {
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    assert!(
        fs::metadata(format!("{}-wal", d.path().display()))
            .unwrap()
            .len()
            > 0
    );
    let mut v = replay();
    let s = d
        .snapshot(&root.path().join("backup"), &bundle(), &mut v)
        .unwrap();
    d.connection
        .execute("UPDATE objects SET value='post-snapshot-effect'", [])
        .unwrap();
    let restored = root.path().join("isolated");
    let report = Database::restore_isolated(&restored, OWNER, &s, &mut v).unwrap();
    assert_eq!(report.schema_version, 1);
    let copy = Database::open(&restored, OWNER).unwrap();
    assert_eq!(
        copy.connection
            .query_row("SELECT value FROM objects", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal-data"
    );
    assert!(copy.dispatch_fence().unwrap_err().contains("isolated"));
    assert!(copy.reconciled().is_err());
    assert!(Database::restore_isolated(root.path(), OWNER, &s, &mut v).is_err());
    assert_eq!(
        d.connection
            .query_row("SELECT value FROM objects", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "post-snapshot-effect"
    );
    assert_eq!(v.calls, 2);
}
#[test]
fn approved_forward_migration_records_digest_and_persists_floor_on_reopen() {
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    let p = plan(&d);
    let s = apply(&mut d, &p, root.path()).unwrap();
    assert!(s.directory.join("manifest.json").exists());
    assert_eq!(d.schema_pin().unwrap(), (2, p.to_schema_hash.clone()));
    let stored: String = d
        .connection
        .query_row(
            "SELECT migration_hash FROM schema_migrations WHERE version=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, p.digest().unwrap());
    assert_eq!(
        d.compatibility_floor().unwrap(),
        Some(p.compatibility.clone())
    );
    d.check_code_compatibility(&"b".repeat(64)).unwrap();
    d.check_code_compatibility(&"c".repeat(64)).unwrap();
    assert!(d.check_code_compatibility(&"d".repeat(64)).is_err());
    drop(d);
    let mut reopened = Database::open(root.path(), OWNER).unwrap();
    assert_eq!(reopened.schema_pin().unwrap().0, 2);
    assert_eq!(
        reopened.compatibility_floor().unwrap(),
        Some(p.compatibility.clone())
    );
    assert!(apply(&mut reopened, &p, &root.path().join("again")).is_err());
}
#[test]
fn approval_binds_every_field_and_ordinary_grants_are_not_approval() {
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    let p = plan(&d);
    let digest = p.digest().unwrap();
    let mut changed = vec![];
    let mut x = p.clone();
    x.owner = "other".into();
    changed.push(x);
    let mut x = p.clone();
    x.from_version = 0;
    changed.push(x);
    let mut x = p.clone();
    x.to_version = 3;
    changed.push(x);
    let mut x = p.clone();
    x.from_schema_hash = "d".repeat(64);
    changed.push(x);
    let mut x = p.clone();
    x.to_schema_hash = "d".repeat(64);
    changed.push(x);
    let mut x = p.clone();
    x.sql.push_str(" CREATE TABLE extra(id INTEGER);");
    changed.push(x);
    let mut x = p.clone();
    x.compatibility.current.code_hash = "d".repeat(64);
    changed.push(x);
    let mut x = p.clone();
    x.recovery_plan.push_str(" changed");
    changed.push(x);
    for x in changed {
        assert_ne!(x.digest().unwrap(), digest);
        assert!(
            d.migrate(
                &x,
                b"authenticated-operator-migration-approval",
                &mut Approval {
                    digest: digest.clone()
                },
                &root.path().join("backup"),
                &root.path().join("trial"),
                &bundle(),
                &mut replay()
            )
            .is_err()
        );
    }
    for proof in [b"db.write".as_slice(), b"promotion-approved", b""] {
        assert!(
            d.migrate(
                &p,
                proof,
                &mut Approval {
                    digest: digest.clone()
                },
                &root.path().join("backup"),
                &root.path().join("trial"),
                &bundle(),
                &mut replay()
            )
            .is_err()
        );
    }
    assert_eq!(d.schema_pin().unwrap().0, 1);
    assert!(!root.path().join("backup").exists());
}
#[test]
fn failed_trial_schema_or_sql_keeps_live_code_schema_and_data() {
    for sql in [
        "CREATE TABLE notes(id INTEGER);",
        "DROP TABLE objects;",
        "DELETE FROM objects;",
        "PRAGMA user_version=9;",
        "COMMIT;",
        "CREATE TABLE _coglet_fake(id INTEGER);",
        "CREATE TABLE notes(id INTEGER); INVALID SQL;",
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut d = provision(root.path());
        let mut p = plan(&d);
        p.sql = sql.into();
        let before = d.schema_pin().unwrap();
        assert!(apply(&mut d, &p, root.path()).is_err(), "{sql}");
        assert_eq!(d.schema_pin().unwrap(), before);
        assert!(d.connection.is_autocommit());
        assert_eq!(
            d.connection
                .query_row("SELECT value FROM objects", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "wal-data"
        );
        assert_eq!(d.compatibility_floor().unwrap(), None);
    }
}
#[test]
fn incompatible_previous_or_candidate_is_rejected_before_backup() {
    for previous in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let mut d = provision(root.path());
        let mut p = plan(&d);
        if previous {
            p.compatibility.previous.max = 1;
        } else {
            p.compatibility.current.min = 3;
        }
        assert!(apply(&mut d, &p, root.path()).is_err());
        assert!(!root.path().join("backup").exists());
    }
}
#[test]
fn archive_corruption_owner_mismatch_and_failed_replay_are_rejected() {
    for entry in ["manifest.json", "database.sqlite3", "recovery.json"] {
        let root = tempfile::tempdir().unwrap();
        let mut d = provision(root.path());
        let s = d
            .snapshot(&root.path().join("backup"), &bundle(), &mut replay())
            .unwrap();
        fs::write(s.directory.join(entry), b"corrupt").unwrap();
        assert!(
            Database::restore_isolated(&root.path().join("copy"), OWNER, &s, &mut replay())
                .is_err()
        );
        assert!(!root.path().join("copy").exists());
    }
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    let s = d
        .snapshot(&root.path().join("backup"), &bundle(), &mut replay())
        .unwrap();
    assert!(
        Database::restore_isolated(&root.path().join("copy"), "other", &s, &mut replay()).is_err()
    );
    let mut v = replay();
    v.fail = true;
    assert!(Database::restore_isolated(&root.path().join("copy"), OWNER, &s, &mut v).is_err());
    let held = Database::open(&root.path().join("copy"), OWNER).unwrap();
    assert!(held.dispatch_fence().is_err());
}
#[test]
fn pending_decision_transaction_marker_or_blob_mismatch_blocks_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    d.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        d.snapshot(&root.path().join("backup"), &bundle(), &mut replay())
            .is_err()
    );
    d.connection.execute_batch("ROLLBACK").unwrap();
    let mut b = bundle();
    b.blobs.insert("f".repeat(64), b"different".to_vec());
    assert!(
        d.snapshot(&root.path().join("backup"), &b, &mut replay())
            .is_err()
    );
    let prepared = d.path.parent().unwrap().join("prepared");
    fs::create_dir(&prepared).unwrap();
    fs::write(prepared.join("pending.prepared"), b"pending").unwrap();
    assert!(
        d.snapshot(&root.path().join("backup"), &bundle(), &mut replay())
            .is_err()
    );
    fs::remove_dir_all(prepared).unwrap();
    d.connection
        .execute(
            "INSERT INTO _coglet_markers VALUES(?1,'trace',zeroblob(8),?2,?2,zeroblob(8),1)",
            rusqlite::params![OWNER, "f".repeat(64)],
        )
        .unwrap();
    assert!(
        d.snapshot(&root.path().join("backup"), &bundle(), &mut replay())
            .unwrap_err()
            .contains("alignment")
    );
}
#[test]
fn foreign_key_corruption_refuses_backup() {
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    d.connection.execute_batch("PRAGMA foreign_keys=OFF; CREATE TABLE bad(id TEXT REFERENCES objects(id)); INSERT INTO bad VALUES('missing'); PRAGMA foreign_keys=ON;").unwrap();
    assert!(
        d.snapshot(&root.path().join("backup"), &bundle(), &mut replay())
            .unwrap_err()
            .contains("foreign key")
    );
}
#[test]
fn floor_cannot_be_released_by_a_second_approved_migration() {
    let root = tempfile::tempdir().unwrap();
    let mut d = provision(root.path());
    let p = plan(&d);
    apply(&mut d, &p, root.path()).unwrap();
    let mut next = p.clone();
    next.from_version = 2;
    next.from_schema_hash = d.schema_pin().unwrap().1;
    next.to_version = 3;
    next.compatibility.previous = next.compatibility.current.clone();
    next.compatibility.previous.max = 3;
    next.compatibility.current.max = 3;
    assert!(
        apply(&mut d, &next, root.path())
            .unwrap_err()
            .contains("floor")
    );
}
