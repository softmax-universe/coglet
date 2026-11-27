use super::*;

struct NeverEnterGuest;
impl Engine for NeverEnterGuest {
    fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
        panic!("historical public replay entered a guest")
    }
}

fn fixture() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut send = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    send["manifest"]["version"] = json!("1.0");
    send
}

// Write real historical frames with the original CBOR state hash, not renamed
// current records. The private reader is never accessible to API callers.
fn old_wal(path: &Path, extra: Option<(&str, &str)>, attempted: bool) -> (Value, Value) {
    old_wal_for(fixture(), path, extra, attempted)
}

// Selected surface-0.2 profile with a dotted name the frozen hub grammar rejects.
fn surface02_fixture() -> Value {
    let mut setup = fixture();
    setup["contract_profile"] = json!("surface-0.2");
    setup["manifest"]["subscriptions"] = json!(["softmax.app.view_changed"]);
    setup["inputs"]["events"][0]["kind"] = json!("softmax.app.view_changed");
    setup
}

fn old_wal_for(
    setup: Value,
    path: &Path,
    extra: Option<(&str, &str)>,
    attempted: bool,
) -> (Value, Value) {
    let mut service = RuntimeService::open(path, &setup, 8).unwrap();
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.1");
    if let Some((path, bytes)) = extra {
        old["guest_script"]["writes"][path] = json!(bytes);
    }
    let record = historical_scope(true, || {
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        let record = service
            .state
            .runtime
            .step(
                &old["inputs"],
                &mut MockEngine::new(old["guest_script"].clone()),
            )
            .unwrap()["journal"]
            .clone();
        if attempted {
            let mut inputs = old["inputs"].clone();
            inputs["step"] = json!(1);
            inputs["previous"] = record["record_hash"].clone();
            for event in inputs["events"].as_array().unwrap() {
                service.state.router.enqueue(event.clone()).unwrap();
            }
            service.state.runtime.unresolved_attempt = Some(inputs);
            service.state.fenced_staged_prefix = Some(0);
        }
        service
            .store
            .commit(
                &serde_json::to_value(&service.state).unwrap(),
                Some(&record),
            )
            .unwrap();
        record
    });
    (setup, record)
}

#[test]
fn disk_store_upgrades_cbor_named_trees() {
    if let Some(path) = std::env::var_os("COGLET_HASH_UPGRADE_CHILD_WAL") {
        let service = RuntimeService::open(Path::new(&path), &fixture(), 8).unwrap();
        assert_eq!(service.state.completed_segments.len(), 1);
        assert_eq!(service.state.runtime.manifest["version"], "1.0");
        assert_eq!(service.store.records().len(), 2);
        assert_eq!(service.store.records()[0]["version"], "0.1");
        assert_eq!(service.store.records()[1]["version"], "1.0");
        println!("fresh-process-upgrade-verified");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let (setup, record) = old_wal(&path, None, false);
    let before = std::fs::read(&path).unwrap();
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.store.records(), std::slice::from_ref(&record));
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    assert_eq!(service.state.completed_segments.len(), 1);
    assert!(service.state.completed_segments[0].hash_upgrade);
    assert_eq!(service.state.runtime.step, 0);
    assert_eq!(service.state.runtime.previous, Value::Null);
    assert_eq!(
        service.state.runtime.time,
        record["logical_time"].as_u64().unwrap()
    );
    let old = service.export_segment(&record["trace_id"]).unwrap();
    assert_eq!(old["version"], "0.1");
    assert_eq!(old["records"][0], record);
    assert!(Runtime::import_trace(&old).is_err());
    assert!(
        Runtime::from_fixture(&setup)
            .unwrap()
            .replay(std::slice::from_ref(&record), &mut NeverEnterGuest)
            .is_err()
    );
    let live_id = service.state.runtime.trace_id.clone().unwrap();
    let live = service.state.runtime.export_trace(&live_id).unwrap();
    assert_eq!(live["version"], "1.0");
    assert_ne!(live["initial_hash"], old["final_hash"]);
    assert_eq!(live["initial_files"], old["records"][0]["output_files"]);
    Runtime::import_trace(&live).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(&before));
    drop(service);
    let upgraded = std::fs::read(&path).unwrap();
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.completed_segments.len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), upgraded);
    let mut inputs = setup["inputs"].clone();
    inputs["trace_id"] = live_id;
    inputs["logical_time"] = record["logical_time"].clone();
    let current = service
        .state
        .runtime
        .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
        .unwrap()["journal"]
        .clone();
    assert_eq!(current["version"], "1.0");
    service
        .store
        .commit(
            &serde_json::to_value(&service.state).unwrap(),
            Some(&current),
        )
        .unwrap();
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.runtime.records(), &[current]);
    assert_eq!(service.store.records()[0], record);
    drop(service);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "execution::hash_upgrade_tests::disk_store_upgrades_cbor_named_trees",
            "--nocapture",
        ])
        .env("COGLET_HASH_UPGRADE_CHILD_WAL", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("fresh-process-upgrade-verified"));
}

#[test]
fn hash_upgrade_defers_unknown_attempt_then_reconciles_without_guest_reentry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let (setup, _) = old_wal(&path, None, true);
    let before = std::fs::read(&path).unwrap();
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert!(service.state.runtime.unresolved_attempt.is_some());
    assert!(service.state.completed_segments.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let inputs = service.state.runtime.unresolved_attempt.clone().unwrap();
    assert!(
        service
            .state
            .runtime
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .is_err()
    );
    let record = historical_scope(true, || {
        let mut original = service.state.runtime.clone();
        original.unresolved_attempt = None;
        original
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .unwrap()["journal"]
            .clone()
    });
    let mut wrong = record.clone();
    wrong["seed"] = json!(999);
    assert!(service.recover_attempt(&wrong).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    service.recover_attempt(&record).unwrap();
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    assert_eq!(service.state.completed_segments.len(), 1);
    assert_eq!(service.queued(), 0);
    assert_eq!(service.store.records().len(), 2);
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.completed_segments.len(), 1);
}

#[cfg(feature = "native-engine")]
#[test]
fn integration_detached_legacy_recovery_routes_through_hash_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-detached.wal");
    let (setup, first) = old_wal(&path, None, true);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let inputs = service.state.runtime.unresolved_attempt.clone().unwrap();
    let record = historical_scope(true, || {
        let mut observer = service.state.runtime.clone();
        observer.unresolved_attempt = None;
        observer
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .unwrap()["journal"]
            .clone()
    });
    service.recover_detached_attempt(&record).unwrap();
    assert!(!hash_upgrade::historical());
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    assert!(service.state.runtime.unresolved_attempt.is_none());
    assert_eq!(service.queued(), 0);
    assert_eq!(service.store.records(), &[first, record]);
    assert_eq!(service.state.completed_segments.len(), 1);
    assert!(service.state.completed_segments[0].hash_upgrade);
    drop(service);
    let reopened = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(reopened.state.runtime.manifest["version"], "1.0");
    assert_eq!(reopened.state.completed_segments.len(), 1);
}

#[test]
fn integration_restore_maps_legacy_root_and_restores_cow_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-restore.wal");
    let (setup, first) = old_wal(&path, None, true);
    let store = DurableStore::open(&path).unwrap();
    let configured = Runtime::from_fixture(&setup).unwrap();
    let catalog = TrustedExtensionCatalog::default();
    let saved = store.state().unwrap();
    let state =
        RuntimeService::restore(&store, &configured, saved, &catalog, &BTreeMap::new()).unwrap();
    assert_eq!(state.runtime.records(), std::slice::from_ref(&first));
    let mut cloned = state.runtime.clone();
    assert!(std::sync::Arc::ptr_eq(
        &state.runtime.records,
        &cloned.records
    ));
    std::sync::Arc::make_mut(&mut cloned.records).clear();
    assert_eq!(state.runtime.records(), std::slice::from_ref(&first));
    assert!(cloned.records().is_empty());
    let mut wrong = configured;
    wrong.initial_setup["manifest"]["version"] = json!("1.1");
    let error = RuntimeService::restore(&store, &wrong, saved, &catalog, &BTreeMap::new())
        .err()
        .unwrap();
    assert_eq!(error, "recovery ABI version mapping differs");
}

#[test]
fn hash_upgrade_rejects_changed_root_and_unmappable_final_tree_without_append() {
    for unmappable in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.wal");
        let (mut setup, _) = old_wal(
            &path,
            unmappable.then_some(("/data/.git/config", "01")),
            false,
        );
        if !unmappable {
            setup["manifest"]["budgets"]["host_bytes"] = json!(123);
        }
        let before = std::fs::read(&path).unwrap();
        let error = RuntimeService::open(&path, &setup, 8).err().unwrap();
        if unmappable {
            assert!(error.contains("invalid path"), "{error}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn hash_upgrade_public_paths_reject_historical_versions() {
    let setup = fixture();
    for version in ["0.1", "0.2"] {
        let mut old = setup.clone();
        old["manifest"]["version"] = json!(version);
        assert!(Runtime::from_fixture(&old).is_err());
        assert!(crate::admit_manifest(&old["manifest"], &json!([]), &json!({})).is_err());
    }
    assert!(!hash_upgrade::historical());
    let _ = std::panic::catch_unwind(|| historical_scope(true, || panic!("scope probe")));
    assert!(!hash_upgrade::historical());
}

#[test]
fn hash_upgrade_database_keeps_exact_historical_record_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("database.wal");
    let mut setup = fixture();
    setup["manifest"]["version"] = json!("1.1");
    setup["manifest"]["database"] =
        json!({"schema_min":1,"schema_max":1,"catalog_hash":"a".repeat(64)});
    setup["initial_db_revision"] = json!(3);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.2");
    let (record, raw) = historical_scope(true, || {
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        let mut record = service
            .state
            .runtime
            .step(
                &old["inputs"],
                &mut MockEngine::new(old["guest_script"].clone()),
            )
            .unwrap()["journal"]
            .clone();
        record["version"] = json!("0.2");
        record["db_before"] = json!(3);
        record["db_after"] = json!(3);
        record["db_schema_version"] = json!(1);
        record["db_catalog_hash"] = json!("a".repeat(64));
        record.as_object_mut().unwrap().remove("record_hash");
        record["record_hash"] = json!(digest(&cbor(&record).unwrap()));
        service.state.runtime.previous = record["record_hash"].clone();
        service.state.runtime.records = vec![record.clone()].into();
        let raw = cbor(&record).unwrap();
        service
            .store
            .commit_database(&serde_json::to_value(&service.state).unwrap(), &raw)
            .unwrap();
        (record, raw)
    });
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.runtime.manifest["version"], "1.1");
    assert_eq!(service.state.runtime.db_revision, Some(3));
    assert_eq!(service.store.database_record_bytes(0), Some(raw.as_slice()));
    let archive = service.export_segment(&record["trace_id"]).unwrap();
    assert_eq!(archive["version"], "0.2");
    assert_eq!(archive["blobs"][digest(&raw)], hex::encode(&raw));
    assert!(Runtime::import_trace(&archive).is_err());
    let live = service
        .state
        .runtime
        .export_trace(service.state.runtime.trace_id.as_ref().unwrap())
        .unwrap();
    assert_eq!(live["version"], "1.1");
    Runtime::import_trace(&live).unwrap();
    drop(service);
    let before = std::fs::read(&path).unwrap();
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.completed_segments.len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(service.store.database_record_bytes(0), Some(raw.as_slice()));
}

// A recovery integration fixture, not old guest admission: create exactly the
// durable legacy PREPARED envelope and SQLite decision state a retired host
// left behind. All step/filesystem hashes are produced in the private old mode;
// no retained journal bytes are converted or rewritten.
#[cfg(feature = "native-engine")]
fn old_database_preparation(
    root: &Path,
    committed: bool,
) -> (Value, Vec<u8>, Vec<u8>, std::path::PathBuf) {
    let path = root.join("pending.wal");
    let mut setup = fixture();
    setup["manifest"]["version"] = json!("1.1");
    setup["manifest"]["database"] =
        json!({"schema_min":1,"schema_max":1,"catalog_hash":"a".repeat(64)});
    setup["initial_db_revision"] = json!(0);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.2");
    old["guest_script"]["writes"]["/data/recovered"] = json!(hex::encode(b"committed"));
    let (record, artifacts) = historical_scope(true, || {
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        service.enqueue(old["inputs"]["events"][0].clone()).unwrap();
        let (inputs, events, mut next) = service.begin_attempt(&old["inputs"]).unwrap().unwrap();
        let mut engine = MockEngine::new(old["guest_script"].clone());
        let mut attempt = DurableAttempt {
            engine: &mut engine,
            store: &mut service.store,
            before: &service.state,
            entered: None,
        };
        let mut result = next.runtime.step(&inputs, &mut attempt).unwrap();
        service.state.runtime.unresolved_attempt = attempt.entered.take();
        service.state.fenced_staged_prefix = Some(0);
        // Assemble the legacy database envelope before it is stored anywhere.
        let record = &mut result["journal"];
        record["version"] = json!("0.2");
        record["db_before"] = json!(0);
        record["db_after"] = json!(1);
        record["db_schema_version"] = json!(1);
        record["db_catalog_hash"] = json!("a".repeat(64));
        record.as_object_mut().unwrap().remove("record_hash");
        record["record_hash"] = json!(digest(&cbor(record).unwrap()));
        next.runtime.previous = record["record_hash"].clone();
        next.runtime.records = vec![record.clone()].into();
        next.runtime.db_revision = Some(1);
        let raw = cbor(record).unwrap();
        coglet_types::decode("database::db-step-record", &raw).unwrap();
        assert!(hash_upgrade::decode_current("database::db-step-record", &raw).is_err());
        RuntimeService::consume_attempt(&mut next, &events, &result, &service.state.runtime.files)
            .unwrap();
        (raw, cbor(&serde_json::to_value(next).unwrap()).unwrap())
    });
    assert!(service.has_unresolved_attempt());
    drop(service);
    let db = coglet_db::Database::open(root, setup["manifest"]["id"].as_str().unwrap()).unwrap();
    let sql = rusqlite::Connection::open(db.path()).unwrap();
    sql.execute_batch("CREATE TABLE proof(value TEXT NOT NULL); PRAGMA user_version=1;")
        .unwrap();
    let projection: Value = coglet_types::decode("database::db-step-record", &record)
        .unwrap()
        .deserialized()
        .unwrap();
    let owner = projection["coglet"].as_str().unwrap();
    let trace = projection["trace_id"].as_str().unwrap();
    let step = projection["step"].as_u64().unwrap();
    let body = format!(
        "COGLET-PREPARED-1\n{owner}\n{trace}\n{step}\n1\n{}\n{}\n{}\n",
        "a".repeat(64),
        hex::encode(&record),
        hex::encode(&artifacts)
    );
    let prepared_dir = db.path().parent().unwrap().join("prepared");
    std::fs::create_dir_all(&prepared_dir).unwrap();
    let prepared = prepared_dir.join(format!(
        "{}.prepared",
        digest(format!("{owner}\n{trace}\n{step}").as_bytes())
    ));
    std::fs::write(&prepared, format!("{body}{}\n", digest(body.as_bytes()))).unwrap();
    if committed {
        sql.execute_batch("BEGIN IMMEDIATE; INSERT INTO proof VALUES ('committed');")
            .unwrap();
        sql.execute(
            "UPDATE _coglet_state SET revision=?1",
            [1u64.to_be_bytes().as_slice()],
        )
        .unwrap();
        sql.execute("INSERT INTO _coglet_markers(coglet,trace_id,step,record_hash,catalog_hash,revision,published) VALUES (?1,?2,?3,?4,?5,?6,0)",
            rusqlite::params![owner, trace, step.to_be_bytes().as_slice(), projection["record_hash"].as_str().unwrap(), "a".repeat(64), 1u64.to_be_bytes().as_slice()]).unwrap();
        sql.execute_batch("COMMIT").unwrap();
    }
    drop((sql, db));
    (setup, record, artifacts, prepared)
}

#[cfg(feature = "native-engine")]
fn recover_old_database_preparation(committed: bool) {
    use crate::database::Publication;
    use coglet_db::{Database, Recovered};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("pending.wal");
    let (setup, raw, artifacts, prepared) = old_database_preparation(&root, committed);
    let pending_bytes = std::fs::read(&prepared).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let mut db = Database::open(&root, setup["manifest"]["id"].as_str().unwrap()).unwrap();
    assert!(service.has_unresolved_attempt());
    assert_eq!(service.state.runtime.manifest["version"], "0.2");
    assert!(service.state.completed_segments.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(db.reconciled().unwrap_err().starts_with("held:"));
    assert!(
        service
            .run_next(&setup["inputs"], &mut NeverEnterGuest)
            .is_err()
    );
    let mut publications = 0;
    let mut unavailable = |kind: Publication, _: &coglet_db::StepId| -> Result<()> {
        assert_eq!(
            kind,
            if committed {
                Publication::Committed
            } else {
                Publication::Aborted
            }
        );
        // Host callbacks never inherit the private historical validator scope.
        assert!(!hash_upgrade::historical());
        let mut old = setup.clone();
        old["manifest"]["version"] = json!("0.2");
        assert!(Runtime::from_fixture(&old).is_err());
        assert!(hash_upgrade::decode_current("database::db-step-record", &raw).is_err());
        publications += 1;
        Err("publication temporarily unavailable".into())
    };
    assert!(
        service
            .recover_database(&mut db, Some(&mut unavailable))
            .unwrap_err()
            .starts_with("held:")
    );
    assert_eq!(publications, 1);
    assert!(service.has_unresolved_attempt());
    assert!(service.state.completed_segments.is_empty());
    assert_eq!(std::fs::read(&prepared).unwrap(), pending_bytes);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop((service, db));
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let mut db = Database::open(&root, setup["manifest"]["id"].as_str().unwrap()).unwrap();
    let recovered = service.recover_database(&mut db, None).unwrap();
    assert!(matches!(
        (&recovered, committed),
        (Recovered::RolledForward(_), true) | (Recovered::Aborted(_), false)
    ));
    assert!(!service.has_unresolved_attempt());
    assert!(!hash_upgrade::historical());
    assert_eq!(service.queued(), 0);
    assert_eq!(service.state.completed_segments.len(), 1);
    assert!(service.state.completed_segments[0].hash_upgrade);
    assert_eq!(service.state.completed_segments[0].end, 1);
    // ADR35's database successor is ABI 1.1 (the plain ABI successor is 1.0).
    assert_eq!(service.state.runtime.manifest["version"], "1.1");
    assert_eq!(service.state.runtime.step, 0);
    assert!(service.state.runtime.previous.is_null());
    assert_eq!(
        service.state.runtime.db_revision,
        Some(u64::from(committed))
    );
    assert_eq!(service.store.records().len(), 1);
    let record = &service.store.records()[0];
    assert_eq!(record["version"], "0.2");
    let published = service.store.database_record_bytes(0).unwrap().to_vec();
    if committed {
        assert_eq!(published, raw);
        assert_eq!(
            service.store.database_artifacts_hash(0),
            Some(digest(&artifacts).as_str())
        );
        assert_eq!(
            service.state.runtime.files.files()["/data/recovered"],
            hex::encode(b"committed")
        );
    } else {
        assert_eq!(record["status"], "aborted");
        assert_eq!(record["failure"]["kind"], "db_recovery_abort");
        let prepared_record: Value = coglet_types::decode("database::db-step-record", &raw)
            .unwrap()
            .deserialized()
            .unwrap();
        assert_eq!(
            record["failure"]["prepared_record_hash"],
            prepared_record["record_hash"]
        );
        assert_eq!(record["output_hash"], record["input_hash"]);
        assert!(
            !service
                .state
                .runtime
                .files
                .files()
                .contains_key("/data/recovered")
        );
    }
    let old_id = service.state.completed_segments[0].trace_id.clone();
    let archive = service.export_segment(&old_id).unwrap();
    assert_eq!(
        archive["blobs"][digest(&published)],
        hex::encode(&published)
    );
    assert!(Runtime::import_trace(&archive).is_err());
    let live_id = service.state.runtime.trace_id.clone().unwrap();
    let live = service.state.runtime.export_trace(&live_id).unwrap();
    assert_eq!(live["initial_files"], record["output_files"]);
    assert_ne!(live["initial_hash"], record["output_hash"]);
    Runtime::import_trace(&live).unwrap();
    let sql = rusqlite::Connection::open(db.path()).unwrap();
    let rows: i64 = sql
        .query_row("SELECT count(*) FROM proof", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, i64::from(committed));
    drop(sql);
    assert!(!prepared.exists());
    db.reconciled().unwrap();
    let after = std::fs::read(&path).unwrap();
    assert!(after.starts_with(&before));
    assert_eq!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Nothing
    );
    assert_eq!(std::fs::read(&path).unwrap(), after);
    drop((service, db));
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.completed_segments.len(), 1);
    assert_eq!(service.state.runtime.trace_id, Some(live_id));
    assert_eq!(
        service.store.database_record_bytes(0),
        Some(published.as_slice())
    );
    assert_eq!(std::fs::read(&path).unwrap(), after);
}

#[cfg(feature = "native-engine")]
#[test]
fn hash_upgrade_recovers_old_database_prepared_commit_before_single_cut() {
    recover_old_database_preparation(true);
}

#[cfg(feature = "native-engine")]
#[test]
fn hash_upgrade_recovers_old_database_prepared_abort_before_single_cut() {
    recover_old_database_preparation(false);
}

#[test]
fn hash_upgrade_preserves_staged_mutations_queue_and_retained_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("staged.wal");
    let (setup, record) = old_wal(&path, None, false);
    let change = json!({"actor":format!("hub:{}", "3".repeat(64)),"target":setup["manifest"]["id"],
        "audit_id":"d".repeat(32),"ticket":null,"method":"debug.write","path":"/data/count","before":"30","after":"31"});
    let retained = json!(hex::encode(b"retained signed archive bytes"));
    let key = digest(b"retained signed archive bytes");
    {
        let mut store = DurableStore::open(&path).unwrap();
        let state = historical_scope(true, || {
            let mut state: ServiceState =
                serde_json::from_value(store.state().unwrap().clone()).unwrap();
            state
                .runtime
                .retained_blobs
                .insert(key.clone(), retained.clone());
            let mut staged = state.runtime.clone();
            staged
                .apply_boundary_changes(std::slice::from_ref(&change))
                .unwrap();
            state.staged_boundary = Some(StagedBoundary {
                changes: vec![change.clone()],
                runtime: staged,
            });
            state
                .router
                .enqueue(setup["inputs"]["events"][0].clone())
                .unwrap();
            state
        });
        store
            .commit(&serde_json::to_value(state).unwrap(), None)
            .unwrap();
    }
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.queued(), 1);
    assert_eq!(service.state.runtime.retained_blobs[&key], retained);
    let staged = service.state.staged_boundary.as_ref().unwrap();
    assert_eq!(staged.changes, vec![change.clone()]);
    assert_eq!(staged.runtime.files.files()["/data/count"], "31");
    assert_eq!(staged.runtime.manifest["version"], "1.0");
    assert_eq!(service.store.records(), &[record]);
    let id = service.state.runtime.trace_id.clone().unwrap();
    let boundary = service.seal_boundary(&id, 20, &[]).unwrap();
    assert_eq!(boundary["version"], "1.0");
    assert_eq!(boundary["boundary"], json!([change]));
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.runtime.files.files()["/data/count"], "31");
    assert_eq!(service.queued(), 1);
}

#[test]
fn hash_upgrade_requires_explicit_cut_discriminator_on_every_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cut.wal");
    let (setup, _) = old_wal(&path, None, false);
    drop(RuntimeService::open(&path, &setup, 8).unwrap());
    {
        let mut store = DurableStore::open(&path).unwrap();
        let mut state = store.state().unwrap().clone();
        state["completed_segments"][0]["hash_upgrade"] = json!(false);
        store.commit(&state, None).unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    assert!(
        RuntimeService::open(&path, &setup, 8)
            .err()
            .unwrap()
            .contains("seal")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn hash_upgrade_empty_predecessor_has_internal_cut_without_abi_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.wal");
    let setup = fixture();
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    historical_scope(true, || {
        let mut old = setup.clone();
        old["manifest"]["version"] = json!("0.1");
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        service
            .store
            .commit(&serde_json::to_value(&service.state).unwrap(), None)
            .unwrap();
    });
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.completed_segments.len(), 1);
    assert_eq!(service.state.completed_segments[0].end, 0);
    assert!(service.store.records().is_empty());
    let old_id = service.state.completed_segments[0].trace_id.clone();
    let archive = service.export_segment(&old_id).unwrap();
    assert_eq!(archive["records"], json!([]));
    assert!(archive["trailing_boundary"].is_null());
    drop(service);
    let before = std::fs::read(&path).unwrap();
    drop(RuntimeService::open(&path, &setup, 8).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn surface02_historical_wal_resumes_with_exact_bytes_and_public_negatives() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = surface02_fixture();
    let (_, record) = old_wal_for(setup.clone(), &path, None, false);
    assert_eq!(record["version"], "0.1");
    assert_eq!(record["events"][0]["kind"], "softmax.app.view_changed");
    let before = std::fs::read(&path).unwrap();
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(
        service.contract_profile(),
        crate::ContractProfile::Surface02
    );
    assert_eq!(service.store.records(), std::slice::from_ref(&record));
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    assert!(service.state.completed_segments[0].hash_upgrade);
    let old = service.export_segment(&record["trace_id"]).unwrap();
    assert_eq!(old["version"], "0.1");
    // Original record and hash are retained verbatim, never rehashed.
    assert_eq!(old["records"][0], record);
    assert_eq!(old["records"][0]["record_hash"], record["record_hash"]);
    assert!(std::fs::read(&path).unwrap().starts_with(&before));
    let live_id = service.state.runtime.trace_id.clone().unwrap();
    let live = service.state.runtime.export_trace(&live_id).unwrap();
    assert_eq!(live["version"], "1.0");
    crate::Runtime::import_trace_with_profile(&live, crate::ContractProfile::Surface02).unwrap();
    drop(service);
    let upgraded = std::fs::read(&path).unwrap();
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.store.records(), std::slice::from_ref(&record));
    assert_eq!(std::fs::read(&path).unwrap(), upgraded);
    // Public negative controls: the private projection is not reachable.
    let surface02 = crate::ContractProfile::Surface02;
    let mut old_setup = setup.clone();
    old_setup["manifest"]["version"] = json!("0.1");
    assert!(Runtime::from_fixture(&old_setup).is_err());
    assert!(surface02.validate_manifest(&old_setup["manifest"]).is_err());
    assert!(surface02.validate("step-record", &record).is_err());
    assert!(
        Runtime::import_trace_with_profile(&old, surface02).is_err(),
        "historical archive is not a public replay input"
    );
    assert!(
        Runtime::from_fixture(&setup)
            .unwrap()
            .replay(std::slice::from_ref(&record), &mut NeverEnterGuest)
            .is_err()
    );
    // Mixed or forged versions are not mapped even inside the private scope.
    historical_scope(true, || {
        surface02.validate_manifest(&old_setup["manifest"]).unwrap();
        surface02.validate("step-record", &record).unwrap();
        let mut forged = record.clone();
        forged["version"] = json!("0.2");
        assert!(surface02.validate("step-record", &forged).is_err());
        let mut name = record.clone();
        name["events"][0]["kind"] = json!("Bad.Name");
        assert!(surface02.validate("step-record", &name).is_err());
        let mut trace = old.clone();
        surface02.validate("trace", &trace).unwrap();
        trace["records"][0]["version"] = json!("1.0");
        assert!(surface02.validate("trace", &trace).is_err());
    });
    assert!(!hash_upgrade::historical());
}

#[test]
fn surface02_historical_unresolved_attempt_reconciles_through_resumed_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let setup = surface02_fixture();
    let (_, _) = old_wal_for(setup.clone(), &path, None, true);
    let before = std::fs::read(&path).unwrap();
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert!(service.state.runtime.unresolved_attempt.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let inputs = service.state.runtime.unresolved_attempt.clone().unwrap();
    let record = historical_scope(true, || {
        let mut original = service.state.runtime.clone();
        original.unresolved_attempt = None;
        original
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .unwrap()["journal"]
            .clone()
    });
    assert_eq!(record["version"], "0.1");
    service.recover_attempt(&record).unwrap();
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    assert_eq!(service.state.completed_segments.len(), 1);
    assert_eq!(service.store.records().len(), 2);
    assert_eq!(service.store.records()[1], record);
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.store.records()[1], record);
}

#[test]
fn surface02_historical_database_record_keeps_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("database.wal");
    let mut setup = surface02_fixture();
    setup["manifest"]["version"] = json!("1.1");
    setup["manifest"]["database"] =
        json!({"schema_min":1,"schema_max":1,"catalog_hash":"a".repeat(64)});
    setup["initial_db_revision"] = json!(3);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.2");
    let (record, raw) = historical_scope(true, || {
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        let mut record = service
            .state
            .runtime
            .step(
                &old["inputs"],
                &mut MockEngine::new(old["guest_script"].clone()),
            )
            .unwrap()["journal"]
            .clone();
        record["version"] = json!("0.2");
        record["db_before"] = json!(3);
        record["db_after"] = json!(3);
        record["db_schema_version"] = json!(1);
        record["db_catalog_hash"] = json!("a".repeat(64));
        record.as_object_mut().unwrap().remove("record_hash");
        record["record_hash"] = json!(digest(&cbor(&record).unwrap()));
        service.state.runtime.previous = record["record_hash"].clone();
        service.state.runtime.records = vec![record.clone()].into();
        let raw = cbor(&record).unwrap();
        service
            .store
            .commit_database(&serde_json::to_value(&service.state).unwrap(), &raw)
            .unwrap();
        (record, raw)
    });
    drop(service);
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(service.state.runtime.manifest["version"], "1.1");
    assert_eq!(service.store.database_record_bytes(0), Some(raw.as_slice()));
    assert_eq!(service.store.records(), std::slice::from_ref(&record));
    let archive = service.export_segment(&record["trace_id"]).unwrap();
    assert_eq!(archive["version"], "0.2");
    assert_eq!(archive["blobs"][digest(&raw)], hex::encode(&raw));
    let surface02 = crate::ContractProfile::Surface02;
    assert!(Runtime::import_trace_with_profile(&archive, surface02).is_err());
    assert!(
        crate::journal::database_projection_with_profile(&raw, surface02).is_err(),
        "public database projection rejects 0.2 bytes"
    );
    historical_scope(true, || {
        assert_eq!(
            crate::journal::database_projection_with_profile(&raw, surface02).unwrap()["record_hash"],
            record["record_hash"]
        );
    });
    drop(service);
    let before = std::fs::read(&path).unwrap();
    let service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(service.store.database_record_bytes(0), Some(raw.as_slice()));
}

// ---- policy producer provenance across the one-way hash upgrade ----------------
// WAL/coordinator unit test with the Mock engine and a historical (ABI 0.1) fixture
// runtime; NOT native WAMR or ADR-0033 daemon acceptance evidence.
#[cfg(feature = "device")]
struct UnknownOutcome;
#[cfg(feature = "device")]
impl Engine for UnknownOutcome {
    fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
        Err("unknown guest outcome".into())
    }
}

#[cfg(feature = "device")]
fn policy_identity(setup: &Value) -> (Value, ReceiptScope) {
    let key = iroh::SecretKey::from_bytes(&[41; 32]);
    let router = key.public().to_string();
    let statement = json!({"purpose":"hub-ref","hub":router,"coglet":setup["manifest"]["id"],
        "ref":"refs/heads/hub","old":null,"new":"a".repeat(40),
        "archive_sha256":"b".repeat(64),"generation":1});
    let signature = key.sign(&coglet_proto::signing::preimage("hub-ref", &statement).unwrap());
    let signed = json!({"statement":statement,"signer":router,"signature":hex::encode(signature.to_bytes())});
    let scope = ReceiptScope {
        actor: "install-actor".into(),
        ticket: None,
        method: "hub.install".into(),
    };
    let identity = json!({"initial":signed,"policy_admission":{"router":router,
        "code":{"signed_ref":signed,"operation_id":"1".repeat(32),"intent":"c".repeat(64),
            "scope":{"actor":"original-code-actor","ticket":null,"method":"hub.git.push"}},
        "install":{"operation_id":"2".repeat(32),"intent":"d".repeat(64),"scope":scope}}});
    (identity, scope)
}

/// Historical managed+policy install with one unknown (pending) or no attempt.
#[cfg(feature = "device")]
fn old_policy_wal(path: &Path, pending: bool) -> Value {
    let mut setup = fixture();
    setup["manifest"]["version"] = json!("1.0");
    setup["initial_files"]
        .as_object_mut()
        .unwrap()
        .retain(|p, _| !p.starts_with("/data/"));
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.1");
    let (identity, scope) = policy_identity(&setup);
    let mut service = RuntimeService::open(path, &setup, 8).unwrap();
    historical_scope(true, || {
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        service
            .managed_install_scoped(&identity, &"2".repeat(32), Some(&scope))
            .unwrap();
        if pending {
            service.managed_start().unwrap();
            service.enqueue(old["inputs"]["events"][0].clone()).unwrap();
            assert!(
                service
                    .run_next(&old["inputs"], &mut UnknownOutcome)
                    .is_err()
            );
        }
    });
    setup
}

#[cfg(feature = "device")]
fn assert_policy_after_upgrade(
    service: &mut RuntimeService,
    target: &str,
    committed: u64,
    attempted: u64,
) -> Value {
    let commit = "a".repeat(40);
    let applied = service.policy_attest_applied(target, 1, &commit).unwrap();
    assert_eq!(applied["code"]["scope"]["actor"], "original-code-actor");
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    service.supervised_now().unwrap();
    let sample = service
        .policy_observe_health(target, 1, &commit, &[], "closed")
        .unwrap();
    assert_eq!(sample["counters"]["attempted"], attempted);
    assert_eq!(sample["counters"]["committed"], committed);
    applied
}

#[cfg(feature = "device")]
#[test]
fn policy_provenance_survives_hash_upgrade_with_pending_attempt_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.wal");
    let setup = old_policy_wal(&path, true);
    let target = setup["manifest"]["id"].as_str().unwrap().to_owned();
    // Historical restore (still ABI 0.1, attempt pending) validates provenance.
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert!(service.state.runtime.unresolved_attempt.is_some());
    assert!(service.state.completed_segments.is_empty());
    let inputs = service.state.runtime.unresolved_attempt.clone().unwrap();
    let record = historical_scope(true, || {
        let mut original = service.state.runtime.clone();
        original.unresolved_attempt = None;
        original
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .unwrap()["journal"]
            .clone()
    });
    service.recover_attempt(&record).unwrap();
    assert_eq!(service.state.completed_segments.len(), 1);
    assert!(service.state.completed_segments[0].hash_upgrade);
    // Immediately after the persisted successor: attestation + counters.
    let applied = assert_policy_after_upgrade(&mut service, &target, 1, 1);
    drop(service);
    // Second WAL reopen under the active current scope.
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let again = assert_policy_after_upgrade(&mut service, &target, 1, 1);
    assert_eq!(again, applied);
    drop(service);
    // Strict public admission: the retained original bytes stay historical.
    assert_eq!(
        RuntimeService::open(&path, &setup, 8)
            .unwrap()
            .state
            .completed_segments[0]
            .initial_setup["manifest"]["version"],
        "0.1"
    );
}

#[cfg(feature = "device")]
#[test]
fn policy_provenance_survives_resolved_hash_upgrade_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.wal");
    let setup = old_policy_wal(&path, false);
    let target = setup["manifest"]["id"].as_str().unwrap().to_owned();
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let applied = assert_policy_after_upgrade(&mut service, &target, 0, 0);
    drop(service);
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    assert_eq!(
        assert_policy_after_upgrade(&mut service, &target, 0, 0),
        applied
    );
}

/// Historical install, step, then a REAL historical code revision (populates a
/// `code_revision` completed segment with a retained ABI 0.1 setup), a post-revision
/// step, and only then the one-way hash upgrade on reopen.
#[cfg(feature = "device")]
fn old_policy_code_revision_wal(path: &Path) -> (Value, CodeRevision) {
    let mut setup = fixture();
    setup["manifest"]["version"] = json!("1.0");
    setup["initial_files"]
        .as_object_mut()
        .unwrap()
        .retain(|p, _| !p.starts_with("/data/"));
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.1");
    let (identity, scope) = policy_identity(&setup);
    let key = iroh::SecretKey::from_bytes(&[41; 32]);
    let statement = json!({"purpose":"hub-ref","hub":key.public().to_string(),
        "coglet":setup["manifest"]["id"],"ref":"refs/heads/hub","old":"a".repeat(40),
        "new":"e".repeat(40),"archive_sha256":"f".repeat(64),"generation":2});
    let signature = key.sign(&coglet_proto::signing::preimage("hub-ref", &statement).unwrap());
    let signed = json!({"statement":statement,"signer":key.public().to_string(),
        "signature":hex::encode(signature.to_bytes())});
    let mut code = old["initial_files"].clone();
    code.as_object_mut()
        .unwrap()
        .retain(|p, _| p.starts_with("/code/"));
    code["/code/revision.txt"] = json!(hex::encode("2"));
    let revision = CodeRevision {
        setup: json!({"manifest": old["manifest"], "initial_files": code}),
        signed_ref: signed,
        operation_id: "3".repeat(32),
        intent: "4".repeat(64),
        scope: ReceiptScope {
            actor: "revision-actor".into(),
            ticket: None,
            method: "hub.git.push".into(),
        },
    };
    let mut service = RuntimeService::open(path, &setup, 8).unwrap();
    historical_scope(true, || {
        service.state.runtime = Runtime::from_fixture(&old).unwrap();
        service
            .managed_install_scoped(&identity, &"2".repeat(32), Some(&scope))
            .unwrap();
        service.managed_start().unwrap();
        service.enqueue(old["inputs"]["events"][0].clone()).unwrap();
        service
            .run_next(
                &old["inputs"],
                &mut MockEngine::new(old["guest_script"].clone()),
            )
            .unwrap()
            .unwrap();
        service.apply_code_revision(&revision, &|| Ok(0)).unwrap();
        let mut again = old["inputs"].clone();
        again["trace_id"] = service.state.runtime.trace_id.clone().unwrap();
        service.enqueue(old["inputs"]["events"][0].clone()).unwrap();
        service
            .run_next(&again, &mut MockEngine::new(old["guest_script"].clone()))
            .unwrap()
            .unwrap();
        assert_eq!(service.state.completed_segments.len(), 1);
        assert!(service.state.completed_segments[0].code_revision.is_some());
        assert_eq!(service.state.runtime.manifest["version"], "0.1");
    });
    (setup, revision)
}

#[cfg(feature = "device")]
#[test]
fn policy_provenance_survives_historical_code_revision_then_hash_upgrade_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.wal");
    let (setup, revision) = old_policy_code_revision_wal(&path);
    let target = setup["manifest"]["id"].as_str().unwrap().to_owned();
    let commit = "e".repeat(40);
    // Reopen: resolved historical state upgrades; code_revision segment is retained
    // with its original ABI 0.1 setup beside the new hash_upgrade segment.
    let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
    let segments = &service.state.completed_segments;
    assert_eq!(segments.len(), 2);
    assert!(segments[0].code_revision.is_some() && !segments[0].hash_upgrade);
    assert_eq!(segments[0].initial_setup["manifest"]["version"], "0.1");
    assert!(segments[1].hash_upgrade && segments[1].code_revision.is_none());
    assert_eq!(service.state.runtime.manifest["version"], "1.0");
    for _ in 0..2 {
        let applied = service.policy_attest_applied(&target, 2, &commit).unwrap();
        assert_eq!(applied["code"]["operation_id"], revision.operation_id);
        assert_eq!(applied["code"]["scope"]["actor"], "revision-actor");
        service.supervised_now().unwrap();
        let sample = service
            .policy_observe_health(&target, 2, &commit, &[], "closed")
            .unwrap();
        // Counters restart at the code cut: exactly the one post-revision step.
        assert_eq!(sample["counters"]["attempted"], 1);
        assert_eq!(sample["counters"]["committed"], 1);
        drop(service);
        // Second real WAL reopen under the active current scope.
        service = RuntimeService::open(&path, &setup, 8).unwrap();
    }
    // Explicit public admission of the historical manifest stays strict.
    let mut old = setup.clone();
    old["manifest"]["version"] = json!("0.1");
    assert!(Runtime::from_fixture(&old).is_err());
    drop(service);
    assert!(RuntimeService::open(&path, &old, 8).is_err());
    let _ = &revision;
}
