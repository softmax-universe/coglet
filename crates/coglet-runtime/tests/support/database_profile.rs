// Trusted naming profile must survive actual native imports, publication,
// restart recovery and offline request reconstruction. DB format stays 0.2.
fn app_database_plan() -> Plan {
    let mut request = coglet_types::decode_profile(&enqueue()).unwrap();
    *entry(request.as_map_mut().unwrap(), "kind") = text("softmax.app.invoke");
    let bytes = coglet_profile_types::encode("surface02::s02-db-enqueue", &request).unwrap();
    vec![vec![("db", statement("put")), ("db", bytes), ("emit", action())]]
}

fn provision_app_database_profile(fx: &mut Fx) {
    fx.setup["contract_profile"] = json!("surface-0.2");
    fx.manifest["required_capabilities"].as_array_mut().unwrap().push(json!("softmax.app.invoke"));
    fx.setup["manifest"] = fx.manifest.clone();
    fx.grants.push("softmax.app.invoke".into());
}

fn app_database_step<E: SharedEngine>(
    fx: &Fx, service: &mut RuntimeService, db: &mut Database, engine: &E,
    gate: Option<JournalGate<'_>>,
) -> Result<Option<Value>, String> {
    let (schema_version, schema_hash) = db.schema_pin().unwrap();
    let pins = Pins {
        code_hash: digest(&fx.code), schema_version, schema_hash,
        catalog_hash: fx.cat.digest().unwrap(),
        grants: Grants { read: true, write: true },
        intent_kinds: ["worker.run".to_string(), "softmax.app.invoke".to_string()].into(),
    };
    let mut context = fx.context();
    context["grants"] = json!(fx.grants);
    service.run_next_database(&context, DatabaseStep {
        engine, code: &fx.code, deployment: Deployment {
            manifest: &fx.manifest, grants: &fx.grants, pins,
        }, db, catalog: &fx.cat, runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536, gate,
    })
}

fn app_database_profile_path<E: SharedEngine>(
    engine: E, mut fx: Fx, runs: Option<Arc<AtomicUsize>>, recovery: u8,
) {
    // Omission cannot be inferred from manifest version or contents.
    let mut unselected = fx.setup.clone();
    unselected["manifest"]["required_capabilities"].as_array_mut().unwrap()
        .push(json!("softmax.app.invoke"));
    assert!(coglet_runtime::Runtime::from_fixture(&unselected).is_err());
    provision_app_database_profile(&mut fx);
    let mut service = fx.service();
    let mut db = fx.db();
    service.enqueue(fx.event(0)).unwrap();
    if recovery == 2 {
        fx.observer().execute_batch(BLOCK_MARKER).unwrap();
    }
    let mut unavailable = |kind: Publication, _: &StepId| -> Result<(), String> {
        assert_eq!(kind, Publication::Committed);
        Err("journal unavailable".into())
    };
    let result = app_database_step(&fx, &mut service, &mut db, &engine,
        (recovery == 1).then_some(&mut unavailable as JournalGate<'_>));
    if recovery != 0 {
        assert!(result.unwrap_err().starts_with("held:"));
        assert!(service.has_unresolved_attempt());
        let entries = guest_runs(&runs);
        drop((service, db));
        if recovery == 2 {
            fx.observer().execute_batch("DROP TRIGGER block_marker").unwrap();
        }
        service = fx.service();
        db = fx.db();
        let recovered = service.recover_database(&mut db, None).unwrap();
        assert!(matches!((recovery, recovered),
            (1, Recovered::RolledForward(_)) | (2, Recovered::Aborted(_))));
        assert_eq!(guest_runs(&runs), entries, "recovery never enters guest");
    } else {
        assert_eq!(result.unwrap().unwrap()["journal"]["status"], "committed");
    }
    assert!(!service.has_unresolved_attempt());
    assert_eq!(service.queued(), 0);
    assert_eq!(fx.count("outbox"), i64::from(recovery != 2));
    if recovery != 2 {
        let kind: String = fx.observer().query_row("SELECT kind FROM outbox", [], |r| r.get(0)).unwrap();
        assert_eq!(kind, "softmax.app.invoke");
    }
    let frames = fx.journal(&service);
    assert_eq!(frames.len(), 1);
    assert!(coglet_types::decode("database::db-step-record", &frames[0]).is_err());
    let record = coglet_profile_types::decode("surface02::s02-db-step-record", &frames[0]).unwrap();
    let calls = field(&record, "calls").as_array().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(field(field(&calls[1], "argument"), "kind").as_text(), Some("softmax.app.invoke"));
    assert_eq!(field(&record, "version").as_text(), Some("0.2"));
    if recovery == 2 {
        assert!(field(&record, "actions").as_array().unwrap().is_empty());
        assert_eq!(field(&record, "input_files"), field(&record, "output_files"));
        assert_eq!(field(&record, "db_before"), field(&record, "db_after"));
    }
    db.dispatch_fence().unwrap();
    drop((service, db));
    service = fx.service();
    assert_eq!(fx.journal(&service), frames, "byte-exact reopen");
    let mut replay = coglet_runtime::Runtime::from_fixture(&fx.setup).unwrap();
    assert_eq!(replay.replay_database(&[&frames[0]], &engine, 8 * 1024 * 1024, 65536).unwrap().len(), 1);
    let mut frozen = fx.setup.clone();
    frozen.as_object_mut().unwrap().remove("contract_profile");
    frozen["manifest"]["required_capabilities"].as_array_mut().unwrap().pop();
    assert!(coglet_runtime::Runtime::from_fixture(&frozen).unwrap()
        .replay_database(&[&frames[0]], &engine, 8 * 1024 * 1024, 65536).is_err());
}

#[test]
fn local_contract_profile_database_mock_commit_recovery_and_replay() {
    for recovery in 0..3 {
        let (engine, fx, runs) = mock_with(app_database_plan(), false);
        app_database_profile_path(engine, fx, runs, recovery);
    }
}
#[test]
fn local_contract_profile_database_wamr_commit_recovery_and_replay() {
    for recovery in 0..3 {
        let code = wasm(&app_database_plan());
        app_database_profile_path(Wamr::interpreter().allow_db(),
            Fx::new(code, coglet_engine_wamr::COMPUTE_MODE), None, recovery);
    }
}
