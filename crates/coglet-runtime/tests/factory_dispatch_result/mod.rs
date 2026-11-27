use super::*;
use coglet_factory_host::{ContractKanbanServer, FactoryHost};

pub(super) fn dispatch_result_consumed(fx: &mut Fx, engine: &Wamr, previous: &Value, before: &str) {
    const TASK: &str = "11111111111111111111111111111111";
    let server = ContractKanbanServer::spawn();
    server.lose_next_create_reply();
    let mut db = fx.db();
    let mut host = FactoryHost::from_adapter(server.adapter());
    let intent: String = fx
        .observer()
        .query_row("SELECT intent_id FROM outbox", [], |r| r.get(0))
        .unwrap();
    host.run(&mut db, 20).unwrap();
    assert_eq!(
        server.len(),
        1,
        "HTTP create committed before response-loss crash"
    );
    assert_eq!(db.get_outbox(&intent).unwrap().unwrap().state, "uncertain");
    drop((host, db));
    let mut db = fx.db();
    let mut host = FactoryHost::from_adapter(server.adapter());
    host.dispatcher()
        .recover_and_dispatch(&mut db, 21, 8)
        .unwrap();
    assert_eq!(
        server.len(),
        1,
        "reconciliation must not create a second remote task"
    );
    let binding = db.get_binding(&intent).unwrap().unwrap_or_else(|| {
        panic!(
            "recovery did not bind original intent: outbox={:?}; requests={}",
            db.get_outbox(&intent).unwrap(),
            serde_json::to_string(&server.requests()).unwrap()
        )
    });
    server.post_result(&binding, true, "Wasm diagnostic done");
    let result = host.run(&mut db, 22).unwrap();
    assert_eq!(result.follow_ups.len(), 1);
    let follow = result.follow_ups[0].clone();
    assert_eq!(follow.kind, "task.completed");
    // Crash after atomic terminal+delivery commit and before guest ingestion.
    drop((host, db));
    let mut db = fx.db();
    let mut host = FactoryHost::from_adapter(server.adapter());
    let replay = host.run(&mut db, 23).unwrap();
    assert_eq!(replay.follow_ups, result.follow_ups);
    assert_eq!(fx.object(TASK).0, "dispatched");
    let mut event = fx.event(2, "result", "active");
    event["kind"] = json!(follow.kind);
    event["correlation_id"] = json!(TASK);
    event["payload"] =
        json!({"task_id":TASK,"attempt":follow.attempt,"lease":follow.lease,"note":follow.note});
    let inbox_id = &digest(follow.event_id.as_bytes())[..32];
    let message = json!({"index":1,"id":inbox_id,"sender":event["source"],"event":event});
    let after = hex::encode(format!("{previous}\n{message}\n"));
    drop(db);
    let mut service = fx.service();
    // This is the existing authenticated embedding boundary fixture, not an
    // authentication claim or a direct task.completed guest injection.
    service.seal_boundary(&fx.setup["inputs"]["trace_id"], 10, &[json!({
        "actor":format!("hub:{}","a".repeat(64)),"audit_id":"99999999999999999999999999999999",
        "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl","before":before,"after":after
    })]).unwrap();
    let next = json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    service.start_segment(&next).unwrap();
    fx.setup["inputs"]["trace_id"] = next;
    // Crash after inbox append and before guest consumption.
    drop(service);
    let mut service = fx.service();
    let mut db = fx.db();
    let replay_start = service
        .export_trace(&fx.setup["inputs"]["trace_id"], 10)
        .unwrap();
    let consumed = fx.step(&mut service, &mut db, engine).unwrap().unwrap();
    assert_eq!(
        consumed["journal"]["status"], "committed",
        "{}",
        consumed["journal"]
    );
    assert_eq!(
        fx.object(TASK).0,
        "completed",
        "actual Wasm must consume the persisted result"
    );
    let version = fx.object(TASK).1;
    drop((service, db)); // crash after guest commit, before host acknowledgement
    let mut service = fx.service();
    let mut db = fx.db();
    let replay = host.run(&mut db, 24).unwrap();
    assert_eq!(replay.follow_ups, result.follow_ups);
    // Persistent runtime input consumption means the same committed message is
    // not scheduled again after reopen. Ack only after observing guest commit.
    assert!(fx.step(&mut service, &mut db, engine).unwrap().is_none());
    assert_eq!(fx.object(TASK).1, version);
    host.dispatcher()
        .acknowledge(&mut db, &follow.event_id)
        .unwrap();
    assert!(host.run(&mut db, 25).unwrap().follow_ups.is_empty());
    let frames = fx.journal(&service);
    drop((service, db));
    std::fs::remove_file(&fx.db_path).unwrap();
    let mut runtime = coglet_runtime::Runtime::import_trace(&replay_start).unwrap();
    let refs: Vec<&[u8]> = frames
        .iter()
        .filter(|frame| {
            let record = coglet_types::decode_profile(frame).unwrap();
            field(&record, "trace_id").as_text() == fx.setup["inputs"]["trace_id"].as_str()
        })
        .map(Vec::as_slice)
        .collect();
    assert_eq!(refs.len(), 1);
    runtime
        .replay_database(&refs, engine, 32 * 1024 * 1024, 65536)
        .unwrap();
    assert!(!fx.db_path.exists());
}
