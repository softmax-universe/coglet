use super::*;
const GOAL: &str = "44444444444444444444444444444444";
fn authorize(h: &mut TestHost) {
    let source = event("factory.goal", 1, []).source;
    Repository::new(h)
        .create_object(&object(
            "factory-goal-authority",
            "policy",
            "active",
            json!({"user_sources":[source],"worker_sources":[]}),
        ))
        .unwrap();
}
fn goal_event(h: &mut TestHost, seq: u64, input: serde_json::Value) -> Event {
    command_event(h, seq, "factory.goal", input)
}
mod acceptance;
mod regressions;
fn command_event(h: &mut TestHost, seq: u64, kind: &str, input: serde_json::Value) -> Event {
    let mut inner = event(kind, seq, []);
    inner.payload = serde_json::from_value(input).unwrap();
    let message_id = format!("{seq:032x}");
    let line = json!({"index":0,"id":message_id,"sender":inner.source,"event":inner});
    h.files
        .insert("/data/inbox.jsonl".into(), format!("{line}\n").into_bytes());
    event(
        "runtime.inbox",
        seq,
        [
            ("index", Value::Integer(0.into())),
            ("id", Value::Text(message_id)),
        ],
    )
}
fn input() -> serde_json::Value {
    json!({"goal_id":GOAL,"description":"find a useful experiment","priority":7,"budget":2,"depends_on":[TASK]})
}
#[test]
fn authenticated_goal_ingestion_has_durable_identity_and_relations() {
    let mut h = TestHost::new();
    authorize(&mut h);
    let wake = goal_event(&mut h, 1, input());
    h.run(wake.clone());
    assert_eq!(h.get(GOAL).0, "open");
    assert_eq!(h.get(GOAL).1["remaining_budget"], 2);
    assert!(h.staged.is_empty());
    let count = h.count("objects");
    h.run(wake);
    assert_eq!(h.count("objects"), count);
    let wake = goal_event(&mut h, 2, input());
    h.run(wake);
    assert_eq!(h.count("objects"), count + 1); // new receipt only
    assert_eq!(h.db.query_row("SELECT count(*) FROM relations WHERE source_id=?1 AND target_id=?2 AND type='depends_on'", [GOAL,TASK], |r| r.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(h.count("outbox"), 0);
    let mut changed = input();
    changed["description"] = "different identity".into();
    let wake = goal_event(&mut h, 3, changed);
    assert!(matches!(
        h.run_failure(wake, false),
        Err(HostError::Conflict)
    ));
    assert_eq!(h.get(GOAL).1["description"], "find a useful experiment");
}
#[test]
fn unauthorized_and_unbound_goal_ingestion_is_refused() {
    let mut h = TestHost::new();
    // No goal authority object: missing trusted configuration fails closed.
    let wake = goal_event(&mut h, 1, input());
    assert!(h.run_failure(wake, false).is_err());
    assert_eq!(h.count("objects"), 2);
    authorize(&mut h);
    let mut wake = goal_event(&mut h, 2, input());
    wake.correlation_id = "f".repeat(32);
    assert!(h.run_failure(wake, false).is_err());
    // Direct injection is not an authenticated goal transport and has no effects.
    let mut direct = event("factory.goal", 3, []);
    direct.payload = serde_json::from_value(input()).unwrap();
    h.run(direct);
    assert!(Repository::new(&mut h).get_object(GOAL).unwrap().is_none());
    h.db.execute(
        "UPDATE objects SET data_json=?1 WHERE id='factory-goal-authority'",
        [json!({"user_sources":[],"worker_sources":[]}).to_string()],
    )
    .unwrap();
    let wake = goal_event(&mut h, 4, input());
    assert!(matches!(
        h.run_failure(wake, false),
        Err(HostError::PermissionDenied)
    ));
}
#[test]
fn invalid_goal_and_failed_step_leave_no_partial_relations() {
    let mut h = TestHost::new();
    authorize(&mut h);
    let before = h.count("objects");
    let wake = goal_event(&mut h, 1, input());
    h.run_failure(wake, true).unwrap();
    assert_eq!(h.count("objects"), before);
    for (seq, bad) in [
        (2, json!({"priority":101})),
        (3, json!({"budget":0})),
        (4, json!({"depends_on":[TASK,TASK]})),
        (5, json!({"unexpected":true})),
    ] {
        let mut payload = input();
        for (k, v) in bad.as_object().unwrap() {
            payload[k] = v.clone();
        }
        let wake = goal_event(&mut h, seq, payload);
        assert!(h.run_failure(wake, false).is_err());
        assert_eq!(h.count("objects"), before);
    }
}
