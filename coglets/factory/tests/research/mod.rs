use super::*;
mod acceptance;
mod backlog;
mod immutability;
mod regressions;
const HYP: &str = "22222222222222222222222222222222";
const EXP: &str = "33333333333333333333333333333333";
const RUN_TASK: &str = "44444444444444444444444444444444";
fn plan() -> serde_json::Value {
    json!({"baseline":"a".repeat(64),"candidate":"b".repeat(64),
        "inputs":"c".repeat(64),"responses":"d".repeat(64),
        "config":"e".repeat(64),"evaluator":"f".repeat(64),"budget":10})
}
fn propose(host: &mut TestHost, seq: u64, ops: serde_json::Value) {
    let mut reply = host.response(
        seq,
        vec![Decision::Replan {
            reason: "placeholder".into(),
        }],
    );
    let (_, root, _) = host.get("factory-executive");
    reply.payload.insert(
        "output".into(),
        Value::Map(vec![(
            Value::Text("text".into()),
            Value::Text(
                json!({"state_version":root["pending"]["state_version"],"operations":ops})
                    .to_string(),
            ),
        )]),
    );
    host.run(reply);
}
fn start() -> TestHost {
    let mut host = TestHost::new();
    Repository::new(&mut host)
        .create_object(&object(
            HYP,
            "hypothesis",
            "open",
            json!({"description":"candidate is better","context_ids":[]}),
        ))
        .unwrap();
    host.run(event(
        "factory.review",
        1,
        [("task_id", Value::Text(HYP.into()))],
    ));
    propose(
        &mut host,
        2,
        json!([
            {"op":"EXPERIMENT","experiment_id":EXP,"task_id":RUN_TASK,"hypothesis_id":HYP,"plan":plan()},
            {"op":"DISPATCH","task_id":RUN_TASK}
        ]),
    );
    host
}
#[test]
fn experiment_uses_existing_durable_dispatch() {
    let host = start();
    assert_eq!(host.get(EXP).0, "running");
    assert_eq!(host.get(RUN_TASK).0, "dispatched");
    assert_eq!(host.count("outbox"), 1);
    assert_eq!(host.get("factory-policy").1["remaining_budget"], 9);
    assert_eq!(host.get("factory-policy").1["available_workers"], 1);
    let (kind, data): (String, String) = host
        .db
        .query_row("SELECT kind,payload_cbor_hex FROM outbox", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(kind, "factory.dispatch");
    assert!(!data.is_empty());
    let n: i64 = host
        .db
        .query_row(
            "SELECT count(*) FROM relations WHERE source_id=?1 AND target_id=?2 AND type='tests'",
            [EXP, HYP],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}
fn report(candidate: Option<i64>) -> serde_json::Value {
    let run = |c: char, code: &str, score: Option<i64>| {
        json!({
            "run_id":c.to_string().repeat(32),"code":code,"plan":plan(),
            "trace":c.to_string().repeat(64),"artifact":"9".repeat(64),
            "cost":5,"score":score
        })
    };
    json!({"experiment_id":EXP,"task_id":RUN_TASK,
        "baseline":run('1',&"a".repeat(64),Some(10)),
        "candidate":run('2',&"b".repeat(64),candidate)})
}
fn finish(host: &mut TestHost, report: serde_json::Value) {
    host.run(event(
        "task.completed",
        3,
        [
            ("task_id", Value::Text(RUN_TASK.into())),
            ("note", Value::Text(report.to_string())),
        ],
    ));
}
#[test]
fn candidate_baseline_comparison_is_reproducible() {
    for (score, expected) in [
        (Some(11), "improvement"),
        (Some(9), "regression"),
        (Some(10), "unchanged"),
        (None, "inconclusive"),
    ] {
        let mut a = start();
        let mut b = start();
        finish(&mut a, report(score));
        finish(&mut b, report(score));
        assert_eq!(a.get(EXP).0, "evaluated");
        let evidence = a.get(EXP).1["evidence_id"].as_str().unwrap().to_owned();
        assert_eq!(a.get(&evidence).1, b.get(&evidence).1);
        assert_eq!(a.get(&evidence).1["comparison"], expected);
    }
}
