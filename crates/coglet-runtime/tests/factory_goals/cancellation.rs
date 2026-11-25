//! Real inbox commands, emitted timers and the ordinary host cancellation path.
use super::*;
use coglet_factory_host::{FactoryHost, MockAdapter};

fn command(
    fx: &mut Fx,
    engine: &Wamr,
    inbox: &mut String,
    index: u64,
    kind: &str,
    payload: Value,
) -> Value {
    let mut event = fx.event(100 + index, "unused", "unused");
    event["kind"] = json!(kind);
    if kind == "llm.reply" {
        event["correlation_id"] = payload["request_id"].clone();
    }
    event["payload"] = payload;
    let message = json!({"index":index,"id":format!("{:032x}",1000+index),"sender":event["source"],"event":event});
    let before = if inbox.is_empty() {
        Value::Null
    } else {
        json!(hex::encode(inbox.as_bytes()))
    };
    inbox.push_str(&format!("{message}\n"));
    let mut service = fx.service();
    service
        .seal_boundary(
            &fx.setup["inputs"]["trace_id"],
            fx.now.get(),
            &[json!({
                "actor":format!("hub:{}","a".repeat(64)),"audit_id":format!("{:032x}",2000+index),
                "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl",
                "before":before,"after":hex::encode(inbox.as_bytes())
            })],
        )
        .unwrap();
    let segment = json!(format!("{:032x}", 3000 + index));
    service.start_segment(&segment).unwrap();
    fx.setup["inputs"]["trace_id"] = segment;
    drop(service);
    let (mut service, mut db) = (fx.service(), fx.db());
    let initial = service
        .export_trace(&fx.setup["inputs"]["trace_id"], fx.now.get())
        .unwrap();
    std::fs::write(
        fx.root.path().join(format!("command-{index}.json")),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    let step = fx.step(&mut service, &mut db, engine).unwrap().unwrap();
    assert_eq!(step["journal"]["status"], "committed", "{step}");
    step
}

fn timers(fx: &Fx, engine: &Wamr, mut step: Value, count: usize) -> Value {
    let (mut service, mut db) = (fx.service(), fx.db());
    for _ in 0..count {
        let at = step["journal"]["timers"][0]["at"]
            .as_u64()
            .expect("review must arm its successor");
        fx.now.set(at);
        step = fx.step(&mut service, &mut db, engine).unwrap().unwrap();
        assert_eq!(step["journal"]["status"], "committed", "{step}");
    }
    step
}

#[test]
fn goal_inbox_deferral_and_cancellation_reaches_normal_host_cycle() {
    let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
    fx.grants.extend([
        "factory.dispatch".into(),
        "factory.cancel".into(),
        "llm.complete".into(),
    ]);
    let engine = Wamr::interpreter().allow_db();
    acceptance::seed(&fx, 1, 5);
    let source = fx.event(100, "unused", "unused")["source"].clone();
    fx.observer()
        .execute(
            "INSERT INTO objects VALUES('factory-goal-authority','policy','active',?1,1,10,10)",
            [json!({"user_sources":[source],"worker_sources":[]}).to_string()],
        )
        .unwrap();
    let (mut service, mut db) = (fx.service(), fx.db());
    let mut inspect = fx.event(1, "unused", "unused");
    inspect["kind"] = json!("factory.inspect");
    inspect["payload"] = json!({"id":"G1"});
    service.enqueue(inspect).unwrap();
    assert_eq!(
        fx.step(&mut service, &mut db, &engine).unwrap().unwrap()["journal"]["status"],
        "committed"
    );
    drop((service, db));
    let mut inbox = String::new();
    let created = command(
        &mut fx,
        &engine,
        &mut inbox,
        0,
        "factory.goal",
        json!({"goal_id":GOAL,"description":"cancel test","priority":9,"budget":1,"depends_on":[]}),
    );
    command(
        &mut fx,
        &engine,
        &mut inbox,
        1,
        "factory.goal_defer",
        json!({"goal_id":GOAL,"until":500}),
    );
    assert_eq!(fx.object(GOAL).0, "deferred");
    let step = timers(&fx, &engine, created, 2);
    assert_eq!(fx.count("outbox"), 0);
    assert_eq!(fx.object(GOAL).0, "deferred");
    timers(&fx, &engine, step, 3);
    assert_eq!(fx.object(GOAL).0, "active");
    assert_eq!(fx.count("outbox"), 1);
    let db = fx.db();
    let dispatch = db.outbox_in_state("pending").unwrap().remove(0);
    drop(db);
    // The actual guest requests a review of the active task before its owning
    // goal is cancelled. The recorded provider reply arrives only afterwards.
    let goal_data: String = fx
        .observer()
        .query_row("SELECT data_json FROM objects WHERE id=?1", [GOAL], |r| {
            r.get(0)
        })
        .unwrap();
    let goal_data: Value = serde_json::from_str(&goal_data).unwrap();
    let task_id = goal_data["active_task"].as_str().unwrap().to_owned();
    let (mut service, mut db) = (fx.service(), fx.db());
    let mut review = fx.event(50, "unused", "unused");
    review["kind"] = json!("factory.review");
    review["payload"] = json!({"task_id":task_id});
    service.enqueue(review).unwrap();
    let reviewed = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(reviewed["journal"]["status"], "committed");
    let reply = recorded_reply(&fx, &reviewed["journal"]["llm_requests"][0]);
    drop((service, db));
    command(
        &mut fx,
        &engine,
        &mut inbox,
        2,
        "factory.goal_cancel",
        json!({"goal_id":GOAL}),
    );
    assert_eq!(fx.object(GOAL).0, "cancelled");
    let stale = command(&mut fx, &engine, &mut inbox, 3, "llm.reply", reply);
    assert!(
        stale["journal"]["llm_requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fx.object(&task_id).0, "dispatched");
    assert_eq!(fx.count("outbox"), 2);
    assert_eq!(
        fx.observer()
            .query_row(
                "SELECT count(*) FROM objects WHERE id=?1",
                ["cccccccccccccccccccccccccccccccc"],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        fx.observer()
            .query_row(
                "SELECT count(*) FROM events WHERE event_type='decision.stale'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let mut db = fx.db();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    let result = host.run(&mut db, fx.now.get() as i64).unwrap();
    assert!(host.dispatcher().adapter.created.is_empty());
    assert_eq!(
        db.get_outbox(&dispatch.intent_id).unwrap().unwrap().state,
        "cancelled"
    );
    assert_eq!(result.follow_ups.len(), 1);
    let result = &result.follow_ups[0];
    drop(db);
    command(
        &mut fx,
        &engine,
        &mut inbox,
        4,
        &result.kind,
        json!({"task_id":result.task_id,"attempt":result.attempt,"lease":result.lease,"note":result.note}),
    );
    assert_eq!(fx.object(&result.task_id).0, "cancelled");
    assert_eq!(acceptance::policy(&fx)["available_workers"], 1);
    assert_eq!(fx.object(GOAL).0, "cancelled");
    let frames = fx.journal(&fx.service());
    std::fs::remove_file(&fx.db_path).unwrap();
    // Cancel, stale provider reply, and durable host result each replay from
    // their sealed immutable inbox boundary without SQLite or a provider.
    for index in 2..=4 {
        let initial: Value = serde_json::from_slice(
            &std::fs::read(fx.root.path().join(format!("command-{index}.json"))).unwrap(),
        )
        .unwrap();
        let trace = format!("{:032x}", 3000 + index);
        let refs: Vec<&[u8]> = frames
            .iter()
            .filter(|frame| {
                let record = coglet_types::decode_profile(frame).unwrap();
                field(&record, "trace_id").as_text() == Some(trace.as_str())
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

fn recorded_reply(fx: &Fx, request: &Value) -> Value {
    use coglet_llm::{Admission, Completion, Gateway, Provider, Route};
    use coglet_mock::llm::{MockLlm, Script};
    let prompt: Value = serde_json::from_str(request["input"]["prompt"].as_str().unwrap()).unwrap();
    let proposal = json!({"state_version":prompt["state_version"],"operations":[
        {"op":"CREATE_GOAL","id":"cccccccccccccccccccccccccccccccc","description":"stale goal","priority":1,"budget":1}]});
    let routes = || {
        [(
            "mock".into(),
            Route {
                provider: "scripted".into(),
                ceiling: 2,
            },
        )]
        .into()
    };
    let path = fx.root.path().join("gateway.sqlite");
    let mut gateway = Gateway::open(path.to_str().unwrap(), routes()).unwrap();
    gateway.set_budget(C, 10).unwrap();
    gateway
        .enqueue_at(
            Admission {
                coglet: C,
                llm_complete_granted: true,
            },
            request,
            1,
        )
        .unwrap();
    drop(gateway);
    let mut gateway = Gateway::open(path.to_str().unwrap(), routes()).unwrap();
    let mut mock = MockLlm::new([Script {
        provider: "scripted".into(),
        request: request.clone(),
        result: Ok(Completion {
            cost: 1,
            reply: json!({"request_id":request["id"],"status":"succeeded","output":{"text":proposal.to_string()}}),
        }),
    }]);
    gateway
        .dispatch_next_checked(|_| Ok(()), |call| mock.complete(call))
        .unwrap();
    assert_eq!(mock.calls.len(), 1);
    drop(gateway);
    let gateway = Gateway::open(path.to_str().unwrap(), routes()).unwrap();
    coglet_types::decode(
        "abi::llm-reply",
        &gateway.replay(C, request["id"].as_str().unwrap()).unwrap(),
    )
    .unwrap()
    .deserialized()
    .unwrap()
}
