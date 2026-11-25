//! Real-WAMR Phase 3 acceptance for the Factory executive: bounded read-only
//! review pages (cursor-bearing), fresh-state admission with committed
//! repository/outbox effects, reopen and offline replay. Goal rows are host
//! fixtures here; authenticated ingestion/cancel transport is proved separately.
use super::*;

const T1: &str = "T1";

fn gid(c: char) -> String {
    format!("{c}{}", "0".repeat(31))
}
fn seed(fx: &Fx, workers: u32, budget: u64) {
    let policy = serde_json::to_string(&factory::executive::Policy {
        model: "mock".into(),
        remaining_budget: budget,
        available_workers: workers,
        max_priority: 100,
    })
    .unwrap();
    let db = fx.observer();
    db.execute(
        "INSERT INTO objects VALUES('factory-policy','policy','active',?1,1,10,10)",
        [policy],
    )
    .unwrap();
}
fn goal(fx: &Fx, c: char, status: &str, priority: u32, deps: &[&str], extra: serde_json::Value) {
    let mut data =
        json!({"description":"goal","priority":priority,"remaining_budget":1,"context_ids":deps});
    for (k, v) in extra.as_object().unwrap() {
        data[k] = v.clone();
    }
    fx.observer()
        .execute(
            "INSERT INTO objects VALUES(?1,'goal',?2,?3,1,10,10)",
            [gid(c), status.to_string(), data.to_string()],
        )
        .unwrap();
}
fn review_step(
    fx: &Fx,
    engine: &Wamr,
    service: &mut coglet_runtime::RuntimeService,
    db: &mut Database,
    seq: u64,
) -> Value {
    fx.now.set(fx.now.get() + 1);
    let mut event = fx.event(seq, "unused", "unused");
    event["kind"] = json!("factory.goal_review");
    event["payload"] = json!({});
    service.enqueue(event).unwrap();
    let step = fx.step(service, db, engine).unwrap().unwrap();
    assert_eq!(step["journal"]["status"], "committed", "{step}");
    step
}
fn status(fx: &Fx, id: &str) -> String {
    fx.observer()
        .query_row("SELECT status FROM objects WHERE id=?1", [id], |r| r.get(0))
        .unwrap()
}
fn tasks(fx: &Fx) -> i64 {
    fx.observer()
        .query_row(
            "SELECT count(*) FROM objects WHERE type='task' AND id NOT IN (?1,'T2')",
            [T1],
            |r| r.get(0),
        )
        .unwrap()
}
fn policy(fx: &Fx) -> serde_json::Value {
    let text: String = fx
        .observer()
        .query_row(
            "SELECT data_json FROM objects WHERE id='factory-policy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
fn fixture() -> (Fx, Wamr) {
    let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
    fx.grants.push("factory.dispatch".into());
    (fx, Wamr::interpreter().allow_db())
}

/// Seven goals exceed one scan page, so selection crosses real cursor-bearing
/// read-only steps and an intervening runtime/DB reopen.
fn priority_scenario(fx: &Fx, engine: &Wamr, first: u64, last: u64) {
    let mut db = fx.db();
    let mut service = fx.service();
    for seq in first..=last {
        review_step(fx, engine, &mut service, &mut db, seq);
    }
}

#[test]
fn goal_budget_and_priority_enforced() {
    let (fx, engine) = fixture();
    seed(&fx, 1, 5);
    for (c, p) in [('a', 1), ('c', 2), ('d', 3), ('e', 4), ('f', 9)] {
        goal(&fx, c, "open", p, &[], json!({}));
    }
    goal(&fx, 'b', "open", 100, &[T1], json!({})); // blocked: T1 is only ready
    // Exhausted per-goal budget: never eligible however high the priority.
    goal(&fx, '9', "open", 100, &[], json!({"remaining_budget":0}));
    fx.observer()
        .execute("INSERT INTO objects VALUES('88888888888888888888888888888888','goal','open','{}',1,10,10)", [])
        .unwrap();
    priority_scenario(&fx, &engine, 1, 12);
    // Reopen mid-way: the continuation and committed state both survive.
    priority_scenario(&fx, &engine, 13, 20);
    assert_eq!(
        status(&fx, &gid('f')),
        "active",
        "highest eligible priority first"
    );
    for c in ['a', 'c', 'd', 'e', 'b', '9'] {
        assert_eq!(status(&fx, &gid(c)), "open", "{c} must not dispatch");
    }
    assert_eq!(fx.count("outbox"), 1);
    assert_eq!(tasks(&fx), 1);
    let p = policy(&fx);
    assert_eq!(
        (
            p["remaining_budget"].as_u64(),
            p["available_workers"].as_u64()
        ),
        (Some(4), Some(0))
    );
}

#[test]
fn goal_replay_does_not_duplicate() {
    let (fx, engine) = fixture();
    seed(&fx, 1, 5);
    goal(&fx, 'a', "open", 5, &[], json!({}));
    goal(&fx, 'b', "open", 7, &[], json!({}));
    let mut db = fx.db();
    let mut service = fx.service();
    // The archive needs one observed database revision: step 1 observes it and
    // the offline replay then covers every later step.
    review_step(&fx, &engine, &mut service, &mut db, 1);
    let initial = service
        .export_trace(&fx.setup["inputs"]["trace_id"], fx.now.get())
        .unwrap();
    for seq in 2..=8 {
        review_step(&fx, &engine, &mut service, &mut db, seq);
    }
    assert_eq!(fx.count("outbox"), 1);
    assert_eq!(status(&fx, &gid('b')), "active");
    // Later review events, after reopening everything, leave one logical effect.
    drop((service, db));
    let (mut service, mut db) = (fx.service(), fx.db());
    for seq in 9..=14 {
        review_step(&fx, &engine, &mut service, &mut db, seq);
    }
    assert_eq!((fx.count("outbox"), tasks(&fx)), (1, 1));
    let after = (fx.count("objects"), fx.count("events"), policy(&fx));
    let frames = fx.journal(&service);
    drop((service, db));
    std::fs::remove_file(&fx.db_path).unwrap();
    let trace = fx.setup["inputs"]["trace_id"].clone();
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
    let refs = &refs[1..];
    assert_eq!(refs.len(), 13);
    coglet_runtime::Runtime::import_trace(&initial)
        .unwrap()
        .replay_database(refs, &engine, 32 * 1024 * 1024, 65536)
        .unwrap();
    assert!(!fx.db_path.exists());
    assert_eq!(after.2["remaining_budget"], 4);
}

#[test]
fn goal_deferral_and_cancellation() {
    let (fx, engine) = fixture();
    seed(&fx, 3, 9);
    goal(&fx, 'a', "deferred", 9, &[], json!({"defer_until":500}));
    goal(&fx, 'b', "open", 1, &[], json!({}));
    goal(&fx, 'c', "cancelled", 100, &[], json!({"cancelled_at":5}));
    let mut db = fx.db();
    let mut service = fx.service();
    for seq in 1..=8 {
        review_step(&fx, &engine, &mut service, &mut db, seq);
    }
    // Deferred (not yet due) and cancelled goals never dispatch.
    assert_eq!(status(&fx, &gid('b')), "active");
    assert_eq!(status(&fx, &gid('a')), "deferred");
    assert_eq!(status(&fx, &gid('c')), "cancelled");
    assert_eq!(fx.count("outbox"), 1);
    // Time passes: the deferred goal resumes, the cancelled one still does not.
    fx.now.set(600);
    for seq in 9..=16 {
        review_step(&fx, &engine, &mut service, &mut db, seq);
    }
    assert_eq!(status(&fx, &gid('a')), "active");
    assert_eq!(status(&fx, &gid('c')), "cancelled");
    assert_eq!(fx.count("outbox"), 2);
    drop((service, db));
    let (mut service, mut db) = (fx.service(), fx.db());
    for seq in 17..=22 {
        review_step(&fx, &engine, &mut service, &mut db, seq);
    }
    assert_eq!((fx.count("outbox"), tasks(&fx)), (2, 2));
}
