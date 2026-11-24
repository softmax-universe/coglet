// Shared property driver: the same request bytes and assertions run through
// actual Wasm imports or the native mock, RuntimeService, WAL and real SQLite.
#[derive(Clone, Copy)]
enum Property { Isolation, Rollback, Version, ForeignKey, Dedup, Bounds, Replay }
fn property_plan(property: Property) -> Plan {
    let request = |name: &str, params: Value| cbor(&json!({"operation":"statement","statement":name,"mode":"write","params":params,"max_rows":64,"max_bytes":65536})).unwrap();
    let calls = match property {
        Property::Isolation | Property::Bounds => vec![("db", statement("scan"))],
        Property::Rollback => vec![("db", statement("put")), ("db", statement("put")), ("db", statement("scan"))],
        Property::Version => vec![("db", statement("put")), ("db", request("stale", json!(["changed","a","obsolete"]))), ("db", statement("scan"))],
        Property::ForeignKey => vec![("db", statement("put")), ("db", request("link", json!(["edge","missing"]))), ("db", statement("scan"))],
        Property::Dedup => vec![("db", enqueue()), ("db", enqueue())],
        Property::Replay => vec![("db", statement("put")), ("db", statement("scan"))],
    };
    vec![calls]
}
fn exercise_property<E: SharedEngine>(property: Property, engine: E, fx: Fx) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    if matches!(property, Property::Isolation) {
        let other = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let mut foreign = Database::open(&fx.root.path().canonicalize().unwrap(), other).unwrap();
        let sql = rusqlite::Connection::open(foreign.path()).unwrap();
        sql.execute_batch("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL); INSERT INTO objects VALUES('b','foreign-secret'); PRAGMA user_version=1;").unwrap();
        assert_ne!(foreign.path(), db.path());
        assert!(fx.step(&mut service, &mut foreign, &engine, None).is_err());
        assert_eq!(service.queued(), 1);
        assert!(service.records().is_empty(), "wrong-owner refusal entered guest");
        fx.observer().execute("INSERT INTO objects VALUES('a','own-secret')", []).unwrap();
    }
    if matches!(property, Property::Bounds) {
        let sql = fx.observer();
        for i in 0..65 { sql.execute("INSERT INTO objects VALUES(?1,'value')", [format!("{i:03}")]).unwrap(); }
    }
    let result = fx.step(&mut service, &mut db, &engine, None).unwrap().unwrap();
    let record = &result["journal"];
    let calls = record["calls"].as_array().unwrap();
    match property {
        Property::Rollback | Property::Version | Property::ForeignKey => {
            assert_eq!(record["status"], "aborted");
            assert_eq!(record["failure"]["kind"], "guest_return");
            assert_eq!(record["failure"]["error"]["code"], "conflict");
            assert_eq!(calls[0]["response"]["affected"], 1);
            assert_eq!(calls[1]["response"]["status"], "failed");
            assert_eq!(calls[2]["result"], 4, "poison ignored by handle=0");
            assert_eq!(fx.count("objects"), 0);
            assert_eq!(fx.count("relations"), 0);
            assert_eq!(record["db_after"], record["db_before"]);
            // Rollback is immediate, not just eventual Unit drop on reopen.
            fx.observer().execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
        }
        Property::Isolation => {
            assert_eq!(record["status"], "committed");
            assert_eq!(calls[0]["response"]["rows"], json!([["a","own-secret"]]));
            assert!(!record.to_string().contains("foreign-secret"));
        }
        Property::Bounds => {
            assert_eq!(record["status"], "committed");
            assert_eq!(calls[0]["response"]["rows"].as_array().unwrap().len(), 64);
            assert!(calls[0]["response"]["next_cursor"].is_string());
            assert!(cbor(&calls[0]["response"]).unwrap().len() <= 65536);
            assert_eq!(fx.count("objects"), 65);
        }
        Property::Dedup => {
            assert_eq!(record["status"], "committed");
            assert_eq!(calls[0]["response"]["duplicate"], false);
            assert_eq!(calls[1]["response"]["duplicate"], true);
            assert_eq!(calls[0]["response"]["intent_id"], calls[1]["response"]["intent_id"]);
            assert_eq!(fx.count("outbox"), 1);
        }
        Property::Replay => {
            assert_eq!(record["status"], "committed");
            assert_eq!(calls[1]["response"]["rows"], json!([["a","value"]]));
        }
    }
    let frames = fx.journal(&service);
    drop((service, db));
    let reopened = fx.service();
    assert_eq!(fx.journal(&reopened), frames);
    drop(reopened);
    // All seven properties additionally require recorded-result execution with
    // the private DB physically absent, not just archive consistency checking.
    std::fs::remove_file(&fx.db_path).unwrap();
    let mut runtime = coglet_runtime::Runtime::from_fixture(&fx.setup).unwrap();
    assert_eq!(runtime.replay_database(&[frames[0].as_slice()], &engine, 8 * 1024 * 1024, 65536).unwrap(), [record["output_hash"].as_str().unwrap()]);
}
macro_rules! property_pair {
    ($real:ident, $mock:ident, $property:ident) => {
        #[test] fn $real() {
            let plan = property_plan(Property::$property);
            let fx = Fx::new(wasm(&plan), coglet_engine_wamr::COMPUTE_MODE);
            exercise_property(Property::$property, Wamr::interpreter().allow_db(), fx);
        }
        #[test] fn $mock() {
            let (engine, fx, _) = mock_with(property_plan(Property::$property), false);
            exercise_property(Property::$property, engine, fx);
        }
    }
}
property_pair!(real_wamr_db_isolation_between_coglets, mock_db_isolation_between_coglets, Isolation);
property_pair!(real_wamr_rollback_on_failed_transaction, mock_rollback_on_failed_transaction, Rollback);
property_pair!(real_wamr_optimistic_version_conflict, mock_optimistic_version_conflict, Version);
property_pair!(real_wamr_fk_integrity_enforced, mock_fk_integrity_enforced, ForeignKey);
property_pair!(real_wamr_outbox_duplicate_idempotency_key, mock_outbox_duplicate_idempotency_key, Dedup);
property_pair!(real_wamr_query_results_bounded, mock_query_results_bounded, Bounds);
property_pair!(real_wamr_db_property_replay_uses_recorded_results, mock_db_property_replay_uses_recorded_results, Replay);
