//! Real-WAMR qualification for the Phase 3 ingestion slice. This is not the
//! complete Phase 3 budget/priority/deferral acceptance suite.
use super::*;
mod acceptance;
mod regressions;
const GOAL: &str = "44444444444444444444444444444444";

#[test]
fn factory_wasm_goal_ingestion_reopen_and_offline_replay() {
    let engine = Wamr::interpreter().allow_db();
    let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
    let mut enclosed = fx.event(1, "unused", "unused");
    enclosed["kind"] = json!("factory.goal");
    enclosed["payload"] = json!({"goal_id":GOAL,"description":"recorded research goal","priority":9,"budget":2,"depends_on":["11111111111111111111111111111111"]});
    fx.observer().execute("INSERT INTO objects VALUES('11111111111111111111111111111111','task','completed','{}',1,10,10)", []).unwrap();
    let policy = serde_json::to_string(&factory::executive::Policy {
        model: "mock".into(),
        remaining_budget: 3,
        available_workers: 1,
        max_priority: 100,
    })
    .unwrap();
    let authority = json!({"user_sources":[enclosed["source"]],"worker_sources":[]}).to_string();
    fx.observer()
        .execute(
            "INSERT INTO objects VALUES('factory-policy','policy','active',?1,1,10,10)",
            [policy],
        )
        .unwrap();
    fx.observer()
        .execute(
            "INSERT INTO objects VALUES('factory-goal-authority','policy','active',?1,1,10,10)",
            [authority],
        )
        .unwrap();
    let message = json!({"index":0,"id":"55555555555555555555555555555555","sender":enclosed["source"],"event":enclosed});
    let mut service = fx.service();
    let mut db = fx.db();
    let mut inspect = fx.event(0, "unused", "unused");
    inspect["kind"] = json!("factory.inspect");
    inspect["payload"] = json!({"id":"G1"});
    service.enqueue(inspect).unwrap();
    let observed = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(observed["journal"]["status"], "committed");
    drop(db);
    service.seal_boundary(&fx.setup["inputs"]["trace_id"], 10, &[json!({
        "actor":format!("hub:{}","a".repeat(64)),"audit_id":"66666666666666666666666666666666",
        "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl",
        "before":null,"after":hex::encode(format!("{message}\n"))
    })]).unwrap();
    let next = json!("77777777777777777777777777777777");
    service.start_segment(&next).unwrap();
    fx.setup["inputs"]["trace_id"] = next;
    drop(service); // boundary committed before host restart
    let mut service = fx.service();
    let mut db = fx.db();
    let replay_start = service
        .export_trace(&fx.setup["inputs"]["trace_id"], 10)
        .unwrap();
    let first = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(first["journal"]["status"], "committed", "{first}");
    assert_eq!(fx.object(GOAL).0, "open");
    assert_eq!(
        fx.observer()
            .query_row(
                "SELECT count(*) FROM relations WHERE source_id=?1 AND type='depends_on'",
                [GOAL],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(fx.count("outbox"), 0);
    assert!(
        first["journal"]["llm_requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let count = fx.count("objects");
    drop((service, db));
    let mut service = fx.service();
    let mut repeated = message.clone();
    repeated["index"] = json!(1);
    repeated["id"] = json!("88888888888888888888888888888888");
    service.seal_boundary(&fx.setup["inputs"]["trace_id"], 10, &[json!({
        "actor":format!("hub:{}","a".repeat(64)),"audit_id":"99999999999999999999999999999999",
        "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl",
        "before":hex::encode(format!("{message}\n")),
        "after":hex::encode(format!("{message}\n{repeated}\n"))
    })]).unwrap();
    let first_segment = fx.setup["inputs"]["trace_id"].clone();
    let next = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    service.start_segment(&next).unwrap();
    fx.setup["inputs"]["trace_id"] = next;
    drop(service);
    let mut service = fx.service();
    let mut db = fx.db();
    let replay_second = service
        .export_trace(&fx.setup["inputs"]["trace_id"], 10)
        .unwrap();
    let second = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(second["journal"]["status"], "committed", "{second}");
    assert_eq!(fx.count("objects"), count);
    let frames = fx.journal(&service);
    drop((service, db));
    std::fs::remove_file(&fx.db_path).unwrap();
    for (trace, initial) in [
        (first_segment, replay_start),
        (fx.setup["inputs"]["trace_id"].clone(), replay_second),
    ] {
        let refs: Vec<&[u8]> = frames
            .iter()
            .filter(|frame| {
                let record = coglet_types::decode_profile(frame).unwrap();
                field(&record, "trace_id").as_text() == trace.as_str()
                    && record
                        .as_map()
                        .unwrap()
                        .iter()
                        .any(|(k, _)| k.as_text() == Some("step"))
            })
            .map(Vec::as_slice)
            .collect();
        assert_eq!(refs.len(), 1);
        coglet_runtime::Runtime::import_trace(&initial)
            .unwrap()
            .replay_database(&refs, &engine, 32 * 1024 * 1024, 65536)
            .unwrap();
    }
    assert!(!fx.db_path.exists());
}
