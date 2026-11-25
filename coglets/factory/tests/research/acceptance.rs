use super::*;
const KNOW: &str = "55555555555555555555555555555555";
fn learn(host: &mut TestHost, seq: u64) {
    propose(host, seq, json!([{"op":"LEARN","knowledge_id":KNOW}]));
}
#[test]
fn evidence_provenance_is_preserved_and_validated() {
    let mut host = start();
    finish(&mut host, report(Some(11)));
    let eid = host.get(EXP).1["evidence_id"].as_str().unwrap().to_string();
    let original = host.get(&eid).1;
    learn(&mut host, 4);
    assert_eq!(host.get(KNOW).1["evidence"], original);
    assert_eq!(host.get(KNOW).0, "improvement");
    assert_eq!(original["origin"]["seq"], 3);
    assert_eq!(original["report"], report(Some(11)));
    for damage in ["missing", "hash", "run", "plan", "experiment"] {
        let mut host = start();
        finish(&mut host, report(Some(11)));
        let eid = host.get(EXP).1["evidence_id"].as_str().unwrap().to_string();
        let mut evidence = host.get(&eid).1;
        if damage == "missing" {
            host.db
                .execute("UPDATE objects SET type='wrong' WHERE id=?1", [&eid])
                .unwrap();
        } else {
            match damage {
                "hash" => evidence["comparison"] = json!("regression"),
                "run" => evidence["report"]["candidate"]["run_id"] = json!("9".repeat(32)),
                "plan" => evidence["plan"]["inputs"] = json!("9".repeat(64)),
                _ => evidence["experiment_id"] = json!(TASK),
            }
            host.db
                .execute(
                    "UPDATE objects SET data_json=?1 WHERE id=?2",
                    [evidence.to_string(), eid],
                )
                .unwrap();
        }
        learn(&mut host, 4);
        let n: i64 = host
            .db
            .query_row(
                "SELECT count(*) FROM objects WHERE type='knowledge'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "{damage}");
    }
}
#[test]
fn failed_experiment_triggers_investigation() {
    let mut host = start();
    host.run(event(
        "task.failed",
        3,
        [("task_id", Value::Text(RUN_TASK.into()))],
    ));
    assert_eq!(host.get(EXP).0, "failed");
    let ops = json!([
        {"op":"INVESTIGATE","task_id":"66666666666666666666666666666666","hypothesis_id":"77777777777777777777777777777777","description":"investigate failed comparison"},
        {"op":"DISPATCH","task_id":"66666666666666666666666666666666"}
    ]);
    propose(&mut host, 4, ops);
    assert_eq!(host.count("outbox"), 2);
    let n: i64 = host
        .db
        .query_row(
            "SELECT count(*) FROM relations WHERE type='informs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    host.run(event(
        "task.failed",
        3,
        [("task_id", Value::Text(RUN_TASK.into()))],
    ));
    host.run(event(
        "task.failed",
        5,
        [("task_id", Value::Text(RUN_TASK.into()))],
    ));
    assert_eq!(host.count("outbox"), 2);
}
