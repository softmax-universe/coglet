// Shared with the two real/mock service tests. The verifier reexecutes the
// pinned guest without a live database and checks the exact prepared state hash.
struct BackupReplay<'a, E> {
    fx: &'a Fx,
    engine: &'a E,
    artifacts_hash: String,
}
impl<E: SharedEngine> coglet_db::RestoreVerifier for BackupReplay<'_, E> {
    fn verify_replay_and_references(
        &mut self,
        owner: &str,
        b: &coglet_db::RecoveryBundle,
    ) -> Result<(), String> {
        if owner != C
            || b.decisions.len() != 1
            || digest(&b.decisions[0].artifacts) != self.artifacts_hash
        {
            return Err("backup identity/artifacts differ from authoritative WAL".into());
        }
        for value in self.fx.setup["initial_files"].as_object().unwrap().values() {
            let raw = hex::decode(value.as_str().unwrap()).unwrap();
            if b.blobs.get(&digest(&raw)) != Some(&raw) {
                return Err("missing referenced input blob".into());
            }
        }
        coglet_runtime::Runtime::from_fixture(&self.fx.setup)?.replay_database(
            &[&b.decisions[0].record],
            self.engine,
            8 * 1024 * 1024,
            65536,
        )?;
        Ok(())
    }
}
fn backup_restore<E: SharedEngine>(engine: E, fx: Fx) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    fx.step(&mut service, &mut db, &engine, None)
        .unwrap()
        .unwrap();
    let record = fx.journal(&service).remove(0);
    drop(service);
    let store = DurableStore::open(&fx.wal).unwrap();
    let artifacts = cbor(store.state().unwrap()).unwrap();
    let hash = store.database_artifacts_hash(0).unwrap().to_owned();
    assert_eq!(digest(&artifacts), hash);
    let blobs = self_blobs(&fx.setup);
    let bundle = coglet_db::RecoveryBundle {
        journal_head: store.records()[0]["record_hash"].as_str().unwrap().into(),
        catalog_hash: fx.cat.digest().unwrap(),
        decisions: vec![coglet_db::ArchivedDecision { record, artifacts }],
        blobs,
    };
    let mut verifier = BackupReplay {
        fx: &fx,
        engine: &engine,
        artifacts_hash: hash,
    };
    let snapshot = db
        .snapshot(&fx.root.path().join("backup"), &bundle, &mut verifier)
        .unwrap();
    let isolated = tempfile::tempdir().unwrap();
    let restored_root = isolated.path().join("restored");
    let report = Database::restore_isolated(&restored_root, C, &snapshot, &mut verifier).unwrap();
    assert_eq!(report.revision, 1);
    let copy = Database::open(&restored_root, C).unwrap();
    assert!(
        copy.dispatch_fence().is_err(),
        "restore never authorizes external dispatch"
    );
    let sql = rusqlite::Connection::open(copy.path()).unwrap();
    let n: i64 = sql
        .query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    // Negative: a missing referenced blob is refused before creating an archive.
    let mut missing = bundle.clone();
    missing.blobs.clear();
    assert!(
        db.snapshot(&fx.root.path().join("bad-backup"), &missing, &mut verifier)
            .is_err()
    );
    assert!(!fx.root.path().join("bad-backup").exists());
    // A snapshot never replaces the live owner's rows or dispatch authority.
    db.dispatch_fence().unwrap();
    assert_eq!(fx.count("outbox"), 1);
}
fn self_blobs(setup: &Value) -> std::collections::BTreeMap<String, Vec<u8>> {
    setup["initial_files"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| {
            let b = hex::decode(v.as_str().unwrap()).unwrap();
            (digest(&b), b)
        })
        .collect()
}
#[test]
fn real_wamr_backup_restore_replays_and_remains_isolated() {
    let (e, f, _) = wamr();
    backup_restore(e, f);
}
#[test]
fn mock_backup_restore_replays_and_remains_isolated() {
    let (e, f, _) = mock();
    backup_restore(e, f);
}
