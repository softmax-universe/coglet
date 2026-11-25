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
    fx.grants
        .extend(["factory.dispatch".into(), "factory.cancel".into()]);
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
    command(
        &mut fx,
        &engine,
        &mut inbox,
        2,
        "factory.goal_cancel",
        json!({"goal_id":GOAL}),
    );
    assert_eq!(fx.object(GOAL).0, "cancelled");
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
        3,
        &result.kind,
        json!({"task_id":result.task_id,"attempt":result.attempt,"lease":result.lease,"note":result.note}),
    );
    assert_eq!(fx.object(&result.task_id).0, "cancelled");
    assert_eq!(acceptance::policy(&fx)["available_workers"], 1);
    assert_eq!(fx.object(GOAL).0, "cancelled");
}
