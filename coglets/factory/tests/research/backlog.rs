use super::*;
#[test]
fn completed_research_is_queued_while_another_decision_is_pending() {
    let mut h = start();
    h.run(task_event("factory.review", 3));
    // A different subject owns the pending provider request when the worker ends.
    h.run(event(
        "task.completed",
        4,
        [
            ("task_id", Value::Text(RUN_TASK.into())),
            ("note", Value::Text(report(Some(11)).to_string())),
        ],
    ));
    let n: i64 =
        h.db.query_row(
            "SELECT count(*) FROM objects WHERE type='deliberation' AND status='pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    let reply = h.response(
        5,
        vec![Decision::Replan {
            reason: "done".into(),
        }],
    );
    h.run(reply);
    for seq in 6..10 {
        let timer = h.timers.last().unwrap().clone();
        let at = timer["at"].as_u64().unwrap();
        let mut wake = event(
            "runtime.timer",
            seq,
            [("id", Value::Text(timer["id"].as_str().unwrap().into()))],
        );
        wake.logical_time = at;
        h.run(wake);
        if h.get("factory-executive").1["pending"]["context"]["subject"]["id"] == EXP {
            break;
        }
    }
    let (_, root, _) = h.get("factory-executive");
    assert_eq!(root["pending"]["context"]["subject"]["id"], EXP);
    assert_eq!(root["pending"]["context"]["purpose"], "evaluation");
}
