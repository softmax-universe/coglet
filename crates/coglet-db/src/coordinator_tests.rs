//! Real SQLite + real process-death tests for the durable step coordinator.
use super::native_tests::{CODE, admission, catalog};
use super::*;
use coglet_types::Value as V;
use std::{cell::Cell as StdCell, process::Command};

const OWNER: &str = "coglet:1111111111111111111111111111111111111111111111111111111111111111";
const TRACE: &str = "55555555555555555555555555555555";

fn provision(root: &Path) -> Database {
    let mut db = Database::open(root, OWNER).unwrap();
    db.provision_for_test("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);INSERT INTO objects VALUES('a','first');PRAGMA user_version=1;").unwrap();
    db
}
fn base_record() -> Vec<(V, V)> {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../spec/fixtures/database/cases.json")).unwrap();
    let raw = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "db-step-record")
        .unwrap()["cbor_hex"]
        .as_str()
        .unwrap()
        .to_string();
    let V::Map(m) = coglet_types::decode_profile(&hex::decode(raw).unwrap()).unwrap() else {
        panic!("record")
    };
    m
}
fn decision(catalog_hash: &str, step: u64, before: u64, after: u64, artifacts: &[u8]) -> Decision {
    decision_from(base_record(), catalog_hash, step, before, after, artifacts)
}
fn legacy_decision(catalog_hash: &str) -> Decision {
    // Exact pre-ADR0035 golden bytes (including historical filesystem hashes).
    let raw = hex::decode(include_str!("legacy_step_record.hex").trim()).unwrap();
    let V::Map(m) = coglet_types_hub0::decode("database::db-step-record", &raw).unwrap() else {
        panic!("legacy record")
    };
    decision_from(m, catalog_hash, 1, 0, 1, ARTIFACTS)
}
fn decision_from(
    mut m: Vec<(V, V)>,
    catalog_hash: &str,
    step: u64,
    before: u64,
    after: u64,
    artifacts: &[u8],
) -> Decision {
    let int = |n: u64| V::Integer(n.into());
    for (k, v) in &mut m {
        let V::Text(name) = k else { continue };
        match name.as_str() {
            "coglet" => *v = V::Text(OWNER.into()),
            "trace_id" => *v = V::Text(TRACE.into()),
            "step" => *v = int(step),
            "db_before" => *v = int(before),
            "db_after" => *v = int(after),
            "db_schema_version" => *v = int(1),
            "db_catalog_hash" => *v = V::Text(catalog_hash.into()),
            "record_hash" => *v = V::Text("0".repeat(64)),
            _ => {}
        }
    }
    let hash =
        coordinator::record_hash_of(&coglet_types::encode_profile(&V::Map(m.clone())).unwrap())
            .unwrap();
    for (k, v) in &mut m {
        if matches!(k, V::Text(t) if t == "record_hash") {
            *v = V::Text(hash.clone());
        }
    }
    Decision {
        record: coglet_types::encode_profile(&V::Map(m)).unwrap(),
        artifacts: artifacts.to_vec(),
    }
}

/// Durable file journal with idempotent, conflict-refusing publication.
struct FileJournal {
    dir: PathBuf,
    fail_commit_once: bool,
    published: usize,
}
impl FileJournal {
    fn new(dir: &Path) -> Self {
        fs::create_dir_all(dir).unwrap();
        Self {
            dir: dir.into(),
            fail_commit_once: false,
            published: 0,
        }
    }
    fn write(&self, name: &str, body: String) -> Result<()> {
        let path = self.dir.join(name);
        if let Ok(existing) = fs::read_to_string(&path) {
            return if existing == body {
                Ok(())
            } else {
                Err("conflicting publication".into())
            };
        }
        let tmp = self.dir.join(format!("{name}.tmp"));
        fs::write(&tmp, body).map_err(|e| e.to_string())?;
        File::open(&tmp)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }
    fn committed(&self, step: u64) -> Option<String> {
        fs::read_to_string(self.dir.join(format!("{step}.committed"))).ok()
    }
    fn aborted(&self, step: u64) -> Option<Vec<u8>> {
        fs::read_to_string(self.dir.join(format!("{step}.aborted")))
            .ok()
            .map(|h| hex::decode(h).unwrap())
    }
}
impl Journal for FileJournal {
    fn publish_committed(&mut self, id: &StepId, record: &[u8], artifacts: &[u8]) -> Result<()> {
        if std::mem::take(&mut self.fail_commit_once) {
            return Err("journal unavailable".into());
        }
        if self.aborted(id.step).is_some() {
            return Err("step already aborted".into());
        }
        self.published += 1;
        self.write(
            &format!("{}.committed", id.step),
            format!("{}\n{}", hex::encode(record), hex::encode(artifacts)),
        )
    }
    fn publish_abort(&mut self, id: &StepId, record: &[u8]) -> Result<()> {
        if self.committed(id.step).is_some() {
            return Err("step already committed".into());
        }
        self.write(&format!("{}.aborted", id.step), hex::encode(record))
    }
}

const ARTIFACTS: &[u8] = b"files+effects+input-consumption";

/// The "guest": runs once, writes through the catalog, then the host completes.
fn run_step(root: &Path, journal: &mut FileJournal) -> Result<StepId> {
    let mut db = Database::open(root, OWNER).unwrap();
    let cat = catalog();
    let admit = admission(&db, &cat);
    let catalog_hash = admit.pins.catalog_hash.clone();
    let mut unit = db.admit(&cat, CODE, admit)?;
    // Guest execution is observable: each run appends exactly one line.
    {
        use std::io::Write;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("guest_runs"))
            .unwrap()
            .write_all(b"run\n")
            .unwrap();
    }
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("d".into()), Cell::Text("second".into())],
        1,
        4096,
    )
    .unwrap();
    if std::env::var("COGLET_DB_MODE").is_ok_and(|m| m == "legacy") {
        coordinator::legacy_complete_for_test(unit, &legacy_decision(&catalog_hash));
        unreachable!();
    }
    unit.complete(&decision(&catalog_hash, 1, 0, 1, ARTIFACTS), journal)
}
fn guest_runs(root: &Path) -> usize {
    fs::read_to_string(root.join("guest_runs"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}
fn count(db: &Database, id: &str) -> i64 {
    db.connection
        .query_row("SELECT COUNT(*) FROM objects WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
}

/// Child entry point: only acts when the parent spawned it. A plain `cargo test`
/// run executes it vacuously; the parent tests assert its observable effects.
#[test]
fn crash_child() {
    let Ok(root) = std::env::var("COGLET_DB_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut journal = FileJournal::new(&root.join("journal"));
    if std::env::var("COGLET_DB_MODE").is_ok_and(|m| m == "recover") {
        let mut db = Database::open(&root, OWNER).unwrap();
        db.recover(&mut journal).unwrap();
    } else {
        run_step(&root, &mut journal).unwrap();
    }
}
fn spawn_child(root: &Path, crash: &str, mode: &str) -> Option<i32> {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "coordinator_tests::crash_child",
            "--test-threads=1",
        ])
        .env("COGLET_DB_ROOT", root)
        .env("COGLET_DB_CRASH", crash)
        .env("COGLET_DB_MODE", mode)
        .output()
        .unwrap()
        .status
        .code()
}

#[test]
fn restart_rolls_forward_when_marker_present() {
    for point in [
        "after_db_commit",
        "after_journal_publish",
        "after_published_mark",
    ] {
        let root = tempfile::tempdir().unwrap();
        drop(provision(root.path()));
        assert_eq!(
            spawn_child(root.path(), point, "step"),
            Some(137),
            "{point}"
        );
        let mut db = Database::open(root.path(), OWNER).unwrap();
        // The decision is committed; admission and dispatch stay fenced until
        // reconciliation finishes.
        assert_eq!(count(&db, "d"), 1);
        assert!(
            db.dispatch_fence().unwrap_err().starts_with(HELD),
            "{point}"
        );
        let cat = catalog();
        let a = admission(&db, &cat);
        assert!(admit_err(db.admit(&cat, CODE, a)).starts_with(HELD));
        let mut journal = FileJournal::new(&root.path().join("journal"));
        let id = StepId {
            coglet: OWNER.into(),
            trace_id: TRACE.into(),
            step: 1,
        };
        assert_eq!(
            db.recover(&mut journal).unwrap(),
            Recovered::RolledForward(id)
        );
        let expected = decision(&cat.digest().unwrap(), 1, 0, 1, ARTIFACTS);
        assert_eq!(
            journal.committed(1).unwrap(),
            format!(
                "{}\n{}",
                hex::encode(&expected.record),
                hex::encode(ARTIFACTS)
            )
        );
        assert!(journal.aborted(1).is_none());
        assert_eq!(db.counters().unwrap().0, 1);
        db.dispatch_fence().unwrap();
        assert_eq!(db.recover(&mut journal).unwrap(), Recovered::Nothing);
        assert_eq!(guest_runs(root.path()), 1, "{point}: guest never rerun");
    }
}

#[test]
fn restart_aborts_authoritatively_when_marker_absent() {
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    assert_eq!(spawn_child(root.path(), "after_prepare", "step"), Some(137));
    let mut db = Database::open(root.path(), OWNER).unwrap();
    assert_eq!(
        count(&db, "d"),
        0,
        "uncommitted write died with the process"
    );
    assert!(db.dispatch_fence().is_err());
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let id = StepId {
        coglet: OWNER.into(),
        trace_id: TRACE.into(),
        step: 1,
    };
    assert_eq!(db.recover(&mut journal).unwrap(), Recovered::Aborted(id));
    assert!(journal.committed(1).is_none());
    let abort = journal.aborted(1).unwrap();
    let a = coglet_types::DatabaseDbStepRecord2::from_cbor(&abort).unwrap();
    let prepared = decision(&catalog().digest().unwrap(), 1, 0, 1, ARTIFACTS);
    assert_eq!(a.status, "aborted");
    assert_eq!(
        a.failure,
        V::Map(vec![
            (V::Text("kind".into()), V::Text("db_recovery_abort".into())),
            (
                V::Text("prepared_record_hash".into()),
                V::Text(
                    coglet_types::DatabaseDbStepRecord1::from_cbor(&prepared.record)
                        .unwrap()
                        .record_hash
                )
            ),
        ])
    );
    assert_eq!((a.db_before, a.db_after), (0, 0));
    assert_eq!(db.counters().unwrap().0, 0);
    db.dispatch_fence().unwrap();
    assert_eq!(db.recover(&mut journal).unwrap(), Recovered::Nothing);
    assert_eq!(guest_runs(root.path()), 1, "abort never reruns the guest");
    // The next step is admitted normally and gets a fresh decision.
    let cat = catalog();
    let a = admission(&db, &cat);
    drop(db.admit(&cat, CODE, a).unwrap());
}

#[test]
fn crash_before_prepare_leaves_nothing_to_recover() {
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    assert_eq!(
        spawn_child(root.path(), "before_prepare", "step"),
        Some(137)
    );
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    assert_eq!(db.recover(&mut journal).unwrap(), Recovered::Nothing);
    assert_eq!(count(&db, "d"), 0);
    assert!(journal.committed(1).is_none() && journal.aborted(1).is_none());
    // Admission attempt accounting survived; no automatic rerun happened.
    assert_eq!(db.counters().unwrap(), (0, 1));
    assert_eq!(guest_runs(root.path()), 1);
    db.dispatch_fence().unwrap();
}

#[test]
fn crash_during_recovery_publication_is_idempotent() {
    for (first, second, rolled_forward) in [
        ("after_prepare", "recover_after_abort_publish", false),
        ("after_db_commit", "recover_after_publish", true),
    ] {
        let root = tempfile::tempdir().unwrap();
        drop(provision(root.path()));
        assert_eq!(spawn_child(root.path(), first, "step"), Some(137));
        assert_eq!(spawn_child(root.path(), second, "recover"), Some(137));
        let mut db = Database::open(root.path(), OWNER).unwrap();
        let mut journal = FileJournal::new(&root.path().join("journal"));
        let got = db.recover(&mut journal).unwrap();
        assert_eq!(matches!(got, Recovered::RolledForward(_)), rolled_forward);
        assert_eq!(journal.committed(1).is_some(), rolled_forward);
        assert_eq!(journal.aborted(1).is_some(), !rolled_forward);
        db.dispatch_fence().unwrap();
        assert_eq!(guest_runs(root.path()), 1);
    }
}

#[test]
fn recovery_abort_overlay_matches_spec() {
    let cat = catalog().digest().unwrap();
    let prepared = decision(&cat, 3, 5, 6, ARTIFACTS);
    let abort = recovery_abort_record(&prepared.record).unwrap();
    let c = coglet_types::DatabaseDbStepRecord1::from_cbor(&prepared.record).unwrap();
    let a = coglet_types::DatabaseDbStepRecord2::from_cbor(&abort).unwrap();
    assert_eq!(a.version, "1.1");
    assert_eq!(a.status, "aborted");
    assert_eq!(a.db_after, a.db_before);
    assert_eq!(a.db_after, 5);
    assert_eq!(a.output_files, c.input_files);
    assert_eq!(a.output_hash, c.input_hash);
    assert!(
        a.actions.is_empty()
            && a.timers.is_empty()
            && a.llm_requests.is_empty()
            && a.logs.is_empty()
    );
    assert_eq!(a.calls, c.calls, "provisional calls are retained");
    assert_eq!(format!("{:?}", a.events), format!("{:?}", c.events));
    assert_eq!(format!("{:?}", a.boundary), format!("{:?}", c.boundary));
    assert_ne!(a.record_hash, c.record_hash);
    assert_eq!(coordinator::record_hash_of(&abort).unwrap(), a.record_hash);
    // The overlay is deterministic, so a crashed replacement retries identically.
    assert_eq!(recovery_abort_record(&prepared.record).unwrap(), abort);
    // Only a valid committed record can be overlaid.
    assert!(recovery_abort_record(&abort).is_err());
    let mut tampered = prepared.record.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert!(recovery_abort_record(&tampered).is_err());
}

#[test]
fn successful_completion_publishes_then_opens_dispatch_fence() {
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    let mut journal = FileJournal::new(&root.path().join("journal"));
    run_step(root.path(), &mut journal).unwrap();
    let mut db = Database::open(root.path(), OWNER).unwrap();
    assert_eq!(journal.published, 1);
    assert_eq!(count(&db, "d"), 1);
    assert_eq!(db.counters().unwrap(), (1, 1));
    assert_eq!(db.unpublished_markers().unwrap(), 0);
    db.dispatch_fence().unwrap();
    assert!(db.pending_files().unwrap().is_empty());
    assert_eq!(db.recover(&mut journal).unwrap(), Recovered::Nothing);
}

#[test]
fn journal_failure_after_commit_holds_and_never_aborts() {
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    let mut journal = FileJournal::new(&root.path().join("journal"));
    journal.fail_commit_once = true;
    let error = run_step(root.path(), &mut journal).unwrap_err();
    assert!(error.starts_with(HELD), "{error}");
    let mut db = Database::open(root.path(), OWNER).unwrap();
    assert_eq!(count(&db, "d"), 1, "committed unit is not undone");
    assert!(db.dispatch_fence().is_err());
    assert!(matches!(
        db.recover(&mut journal).unwrap(),
        Recovered::RolledForward(_)
    ));
    assert!(journal.aborted(1).is_none());
    db.dispatch_fence().unwrap();
}

#[test]
fn inaccessible_marker_store_holds_instead_of_inferring_abort() {
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    assert_eq!(spawn_child(root.path(), "after_prepare", "step"), Some(137));
    let mut db = Database::open(root.path(), OWNER).unwrap();
    db.connection
        .execute_batch("ALTER TABLE _coglet_markers RENAME TO _hidden")
        .unwrap();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let error = db.recover(&mut journal).unwrap_err();
    assert!(error.starts_with(HELD), "{error}");
    assert!(journal.aborted(1).is_none() && !db.halted());
    assert_eq!(db.pending_files().unwrap().len(), 1, "preparation retained");
    db.connection
        .execute_batch("ALTER TABLE _hidden RENAME TO _coglet_markers")
        .unwrap();
    assert!(matches!(
        db.recover(&mut journal).unwrap(),
        Recovered::Aborted(_)
    ));
}

fn crash_committed(root: &Path) {
    drop(provision(root));
    assert_eq!(spawn_child(root, "after_db_commit", "step"), Some(137));
}
fn assert_halted(root: &Path, mut db: Database, error: String) {
    assert!(error.starts_with(CORRUPTION), "{error}");
    assert!(db.halted());
    let mut journal = FileJournal::new(&root.join("journal"));
    assert!(journal.committed(1).is_none() && journal.aborted(1).is_none());
    // The halt is durable: a restarted owner refuses admission, recovery, dispatch.
    drop(db);
    db = Database::open(root, OWNER).unwrap();
    assert!(db.halted());
    let cat = catalog();
    let a = admission(&db, &cat);
    assert!(admit_err(db.admit(&cat, CODE, a)).starts_with(HALTED));
    assert!(db.recover(&mut journal).unwrap_err().starts_with(HALTED));
    assert!(db.dispatch_fence().unwrap_err().starts_with(HALTED));
}

#[test]
fn corruption_missing_preparation_with_committed_marker_halts() {
    let root = tempfile::tempdir().unwrap();
    crash_committed(root.path());
    let mut db = Database::open(root.path(), OWNER).unwrap();
    for f in db.pending_files().unwrap() {
        fs::remove_file(f).unwrap();
    }
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let error = db.recover(&mut journal).unwrap_err();
    assert_halted(root.path(), db, error);
}

#[test]
fn corruption_hash_mismatch_and_torn_preparation_halt() {
    // Preparation for the same step identity but different record than the marker.
    let root = tempfile::tempdir().unwrap();
    crash_committed(root.path());
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let forged = decision(&catalog().digest().unwrap(), 1, 0, 1, b"different");
    let mut other = forged.clone();
    // A different valid record (changed seed) for the same identity.
    let mut m = base_record();
    for (k, v) in &mut m {
        match k {
            V::Text(t) if t == "seed" => *v = V::Integer(99u64.into()),
            V::Text(t) if t == "step" => *v = V::Integer(1u64.into()),
            V::Text(t) if t == "coglet" => *v = V::Text(OWNER.into()),
            V::Text(t) if t == "trace_id" => *v = V::Text(TRACE.into()),
            V::Text(t) if t == "db_before" => *v = V::Integer(0u64.into()),
            V::Text(t) if t == "db_after" => *v = V::Integer(1u64.into()),
            V::Text(t) if t == "db_schema_version" => *v = V::Integer(1u64.into()),
            V::Text(t) if t == "db_catalog_hash" => *v = V::Text(catalog().digest().unwrap()),
            V::Text(t) if t == "record_hash" => *v = V::Text("0".repeat(64)),
            _ => {}
        }
    }
    let hash =
        coordinator::record_hash_of(&coglet_types::encode_profile(&V::Map(m.clone())).unwrap())
            .unwrap();
    for (k, v) in &mut m {
        if matches!(k, V::Text(t) if t == "record_hash") {
            *v = V::Text(hash.clone());
        }
    }
    other.record = coglet_types::encode_profile(&V::Map(m)).unwrap();
    coordinator::forge_prepared(&db, &other).unwrap();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let error = db.recover(&mut journal).unwrap_err();
    assert_halted(root.path(), db, error);

    // A torn/bit-flipped preparation is corruption, never an abort.
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    assert_eq!(spawn_child(root.path(), "after_prepare", "step"), Some(137));
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let file = db.pending_files().unwrap().remove(0);
    let mut raw = fs::read(&file).unwrap();
    let n = raw.len() / 2;
    raw[n] ^= 1;
    fs::write(&file, raw).unwrap();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let error = db.recover(&mut journal).unwrap_err();
    assert_halted(root.path(), db, error);
    let _ = forged;
}

#[test]
fn complete_refuses_inconsistent_or_ineligible_units_before_any_durable_state() {
    let root = tempfile::tempdir().unwrap();
    let mut db = provision(root.path());
    let cat = catalog();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let hash = cat.digest().unwrap();
    // No DB call: ordinary ABI commit is the path.
    let a = admission(&db, &cat);
    let unit = db.admit(&cat, CODE, a).unwrap();
    assert!(
        unit.complete(&decision(&hash, 1, 0, 0, b""), &mut journal)
            .unwrap_err()
            .contains("ordinary ABI")
    );
    // Wrong revision, wrong catalog, wrong owner, unsigned hash: nothing prepared.
    let mut bad = vec![
        decision(&hash, 1, 0, 0, b""), // write claims no revision advance
        decision(&"b".repeat(64), 1, 0, 1, b""), // other catalog
        decision(&hash, 1, 9, 10, b""), // stale db_before
    ];
    let mut unsigned = decision(&hash, 1, 0, 1, b"");
    *unsigned.record.last_mut().unwrap() ^= 1;
    bad.push(unsigned);
    for d in bad {
        let a = admission(&db, &cat);
        let mut unit = db.admit(&cat, CODE, a).unwrap();
        unit.statement(
            "put",
            Mode::Write,
            &[Cell::Text("x".into()), Cell::Text("y".into())],
            1,
            4096,
        )
        .unwrap();
        assert!(unit.complete(&d, &mut journal).is_err());
        assert!(db.pending_files().unwrap().is_empty());
        assert_eq!(db.unpublished_markers().unwrap(), 0);
        assert_eq!(count(&db, "x"), 0, "rolled back");
    }
    // A poisoned unit cannot complete (txn_rollback_on_failure keeps its own test).
    let a = admission(&db, &cat);
    let mut unit = db.admit(&cat, CODE, a).unwrap();
    let p = [Cell::Text("a".into()), Cell::Text("dup".into())];
    assert!(unit.statement("put", Mode::Write, &p, 1, 4096).is_err());
    assert!(
        unit.complete(&decision(&hash, 1, 0, 1, b""), &mut journal)
            .is_err()
    );
    assert_eq!(db.counters().unwrap().0, 0);
    assert_eq!(journal.published, 0);
}

#[test]
fn read_only_step_keeps_revision_and_commits_cursor_with_marker() {
    let root = tempfile::tempdir().unwrap();
    let mut db = provision(root.path());
    db.provision_for_test("INSERT INTO objects VALUES('b','second')")
        .unwrap();
    let cat = catalog();
    let hash = cat.digest().unwrap();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let a = admission(&db, &cat);
    let mut unit = db.admit(&cat, CODE, a).unwrap();
    let page = unit
        .statement("scan", Mode::Read, &[Cell::Text("a".into())], 1, 65536)
        .unwrap();
    assert!(page.next_cursor.is_some());
    unit.complete(&decision(&hash, 1, 0, 0, b""), &mut journal)
        .unwrap();
    assert_eq!(
        db.counters().unwrap().0,
        0,
        "read-only does not advance revision"
    );
    assert_eq!(db.unpublished_markers().unwrap(), 0);
    let markers: i64 = db
        .connection
        .query_row("SELECT COUNT(*) FROM _coglet_markers", [], |r| r.get(0))
        .unwrap();
    let cursors: i64 = db
        .connection
        .query_row("SELECT COUNT(*) FROM _coglet_cursors", [], |r| r.get(0))
        .unwrap();
    assert_eq!((markers, cursors), (1, 1));
}

#[test]
fn dispatcher_mutations_share_fence_and_revision() {
    let root = tempfile::tempdir().unwrap();
    drop(provision(root.path()));
    assert_eq!(
        spawn_child(root.path(), "after_db_commit", "step"),
        Some(137)
    );
    let mut db = Database::open(root.path(), OWNER).unwrap();
    assert!(
        db.dispatcher_mutation(|_| Ok(()))
            .unwrap_err()
            .starts_with(HELD)
    );
    let mut journal = FileJournal::new(&root.path().join("journal"));
    db.recover(&mut journal).unwrap();
    let ran = StdCell::new(false);
    assert_eq!(
        db.dispatcher_mutation(|_| {
            ran.set(true);
            Ok(())
        })
        .unwrap(),
        2
    );
    assert!(ran.get());
    assert_eq!(db.counters().unwrap().0, 2);
    // A failing mutation rolls back and does not advance the revision.
    assert!(db.dispatcher_mutation(|_| Err("no".into())).is_err());
    assert_eq!(db.counters().unwrap().0, 2);
}

fn admit_err(result: Result<Unit<'_>>) -> String {
    match result {
        Err(e) => e,
        Ok(_) => panic!("admission unexpectedly succeeded"),
    }
}

#[test]
fn legacy_prepared_crash_recovery_preserves_bytes_and_abort_hashes() {
    for (point, committed) in [("after_prepare", false), ("after_db_commit", true)] {
        let root = tempfile::tempdir().unwrap();
        drop(provision(root.path()));
        assert_eq!(spawn_child(root.path(), point, "legacy"), Some(137));
        assert_eq!(
            spawn_child(
                root.path(),
                if committed {
                    "recover_after_publish"
                } else {
                    "recover_after_abort_publish"
                },
                "recover"
            ),
            Some(137)
        );
        let mut db = Database::open(root.path(), OWNER).unwrap();
        let expected = legacy_decision(&catalog().digest().unwrap());
        let original = coordinator::validate_stored_committed(&expected.record).unwrap();
        assert_eq!(original.version, "0.2");
        assert!(
            recovery_abort_record(&expected.record).is_err(),
            "public overlay is current-only"
        );
        let mut journal = FileJournal::new(&root.path().join("journal"));
        let result = db.recover(&mut journal).unwrap();
        assert_eq!(matches!(result, Recovered::RolledForward(_)), committed);
        assert_eq!(count(&db, "d"), i64::from(committed));
        if committed {
            assert_eq!(
                journal.committed(1).unwrap(),
                format!(
                    "{}\n{}",
                    hex::encode(&expected.record),
                    hex::encode(ARTIFACTS)
                )
            );
        } else {
            let abort = journal.aborted(1).unwrap();
            let value = coglet_types_hub0::decode("database::db-step-record", &abort).unwrap();
            let a: coglet_types::DatabaseDbStepRecord2 = value.deserialized().unwrap();
            assert_eq!(a.version, "0.2");
            assert_eq!(a.output_hash, original.input_hash);
            assert_eq!(a.input_hash, original.input_hash);
            assert_eq!(a.before_hash, original.before_hash);
            assert_eq!(a.output_files, original.input_files);
            assert_eq!((a.db_before, a.db_after), (0, 0));
            assert_eq!(a.record_hash, coordinator::record_hash_of(&abort).unwrap());
            assert!(
                matches!(&a.failure, V::Map(m) if m.iter().any(|(k,v)| matches!(k, V::Text(s) if s == "prepared_record_hash") && v == &V::Text(original.record_hash.clone())))
            );
        }
        assert_eq!(guest_runs(root.path()), 1);
        db.dispatch_fence().unwrap();
        assert_eq!(db.recover(&mut journal).unwrap(), Recovered::Nothing);
    }
}

#[test]
fn stored_legacy_validation_rejects_unknown_fields_and_bad_hashes() {
    let original = legacy_decision(&catalog().digest().unwrap());
    let V::Map(mut fields) = coglet_types::decode_profile(&original.record).unwrap() else {
        panic!("record")
    };
    fields.push((V::Text("unrecognized".into()), V::Null));
    let forged = decision_from(fields, &catalog().digest().unwrap(), 1, 0, 1, ARTIFACTS);
    assert!(
        coordinator::validate_stored_committed(&forged.record).is_err(),
        "a correct hash cannot bypass the frozen closed schema"
    );
    let V::Map(mut fields) = coglet_types::decode_profile(&original.record).unwrap() else {
        panic!("record")
    };
    for (k, v) in &mut fields {
        if matches!(k, V::Text(s) if s == "record_hash") {
            *v = V::Text("0".repeat(64));
        }
    }
    assert!(
        coordinator::validate_stored_committed(
            &coglet_types::encode_profile(&V::Map(fields)).unwrap()
        )
        .is_err()
    );
}

#[test]
fn legacy_decision_is_rejected_before_public_completion_persists_anything() {
    let root = tempfile::tempdir().unwrap();
    let mut db = provision(root.path());
    let cat = catalog();
    let old = legacy_decision(&cat.digest().unwrap());
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let a = admission(&db, &cat);
    let mut unit = db.admit(&cat, CODE, a).unwrap();
    unit.statement(
        "put",
        Mode::Write,
        &[Cell::Text("x".into()), Cell::Text("y".into())],
        1,
        4096,
    )
    .unwrap();
    assert!(unit.complete(&old, &mut journal).is_err());
    assert_eq!(count(&db, "x"), 0);
    assert!(db.pending_files().unwrap().is_empty());
    assert_eq!(db.unpublished_markers().unwrap(), 0);
    assert_eq!(journal.published, 0);
}

#[test]
fn recovery_journal_failure_is_held_and_preserves_the_decision() {
    for (point, committed) in [("after_db_commit", true), ("after_prepare", false)] {
        let root = tempfile::tempdir().unwrap();
        drop(provision(root.path()));
        assert_eq!(spawn_child(root.path(), point, "step"), Some(137));
        let mut db = Database::open(root.path(), OWNER).unwrap();
        let mut journal = FileJournal::new(&root.path().join("journal"));
        journal.fail_commit_once = true;
        // Make the abort publication fail too: pre-publish a conflicting record.
        if !committed {
            journal.write("1.committed", "conflict".into()).unwrap();
        }
        let error = db.recover(&mut journal).unwrap_err();
        assert!(error.starts_with(HELD), "{point}: {error}");
        assert!(!db.halted());
        assert_eq!(db.pending_files().unwrap().len(), 1, "preparation kept");
        assert!(journal.aborted(1).is_none());
        if committed {
            assert!(matches!(
                db.recover(&mut journal).unwrap(),
                Recovered::RolledForward(_)
            ));
        }
    }
}
