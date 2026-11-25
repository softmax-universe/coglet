use super::*;
const HIGH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CHILD: &str = "cccccccccccccccccccccccccccccccc";

fn create(h: &mut TestHost, seq: u64, id: &str, priority: u32) {
    let wake = goal_event(
        h,
        seq,
        json!({"goal_id":id,"description":"goal","priority":priority,"budget":2,"depends_on":[]}),
    );
    h.run(wake);
}

// Consume only timers actually emitted by the guest, not fabricated reviews.
fn timer(h: &mut TestHost, seq: u64) {
    let t = h.timers.pop().expect("guest must arm review");
    let mut e = event("runtime.timer", seq, []);
    e.logical_time = t["at"].as_u64().unwrap();
    e.payload = serde_json::from_value(t).unwrap();
    h.run(e);
}

#[test]
fn goal_scan_invalidates_at_cursor_free_status_and_admission_boundaries() {
    for pages in [1, 2] {
        let mut h = TestHost::new();
        authorize(&mut h);
        create(&mut h, 1, GOAL, 1);
        for seq in 10..10 + pages {
            timer(&mut h, seq);
        }
        // The open page is exhausted, or the entire scan has finished. Both
        // boundaries lack a cursor, but must still reject the old winner.
        create(&mut h, 20, HIGH, 99);
        for seq in 30..33 {
            timer(&mut h, seq);
        }
        assert_eq!(h.get(HIGH).0, "active");
        assert_eq!(h.get(GOAL).0, "open");
        assert_eq!(h.count("outbox"), 1);
    }
}

#[test]
fn model_goal_dispatch_cannot_bypass_ranked_admission() {
    for (status, extra) in [
        ("open", json!({"remaining_budget":0})),
        ("deferred", json!({"defer_until":9999})),
        ("open", json!({"context_ids":[TASK]})),
        ("open", json!({})), // even eligible work must use the ranked path
    ] {
        let mut h = TestHost::new();
        authorize(&mut h);
        create(&mut h, 1, GOAL, 1);
        let mut goal = Repository::new(&mut h).get_object(GOAL).unwrap().unwrap();
        let mut data: serde_json::Value = serde_json::from_str(&goal.data_json).unwrap();
        for (k, v) in extra.as_object().unwrap() {
            data[k] = v.clone();
        }
        goal.status = status.into();
        goal.data_json = data.to_string();
        Repository::new(&mut h).update_object(&goal).unwrap();
        h.run(event(
            "factory.review",
            2,
            [("task_id", Value::Text(GOAL.into()))],
        ));
        let reply = h.response(
            3,
            vec![
                Decision::Decompose {
                    task_id: CHILD.into(),
                    description: "bypass".into(),
                },
                Decision::Dispatch {
                    task_id: CHILD.into(),
                },
            ],
        );
        h.run(reply);
        assert_eq!(h.count("outbox"), 0);
        assert!(Repository::new(&mut h).get_object(CHILD).unwrap().is_none());
        assert_eq!(h.get(GOAL).0, status);
        assert_eq!(h.get("factory-policy").1["remaining_budget"], 10);
    }
}

#[test]
fn failed_goal_attempt_rearms_emitted_review_timer() {
    let mut h = TestHost::new();
    authorize(&mut h);
    create(&mut h, 1, GOAL, 1);
    for seq in 10..15 {
        timer(&mut h, seq);
    }
    assert!(
        h.timers.is_empty(),
        "idle scan should stop with only active work"
    );
    let task = h.get(GOAL).1["active_task"].as_str().unwrap().to_owned();
    h.run(event("task.failed", 20, [("task_id", Value::Text(task))]));
    assert_eq!(h.get(GOAL).0, "open");
    for seq in 30..33 {
        timer(&mut h, seq);
    }
    assert_eq!(h.get(GOAL).0, "active");
    assert_eq!(h.count("outbox"), 2);
    assert_eq!(h.get(GOAL).1["remaining_budget"], 0);
}

#[test]
fn model_created_goal_starts_emitted_review_timer() {
    let mut h = TestHost::new();
    h.run(task_event("factory.review", 1));
    let reply = h.response(
        2,
        vec![Decision::CreateGoal {
            id: GOAL.into(),
            description: "new goal".into(),
            priority: 5,
            budget: 1,
        }],
    );
    h.run(reply);
    for seq in 10..13 {
        timer(&mut h, seq);
    }
    assert_eq!(h.get(GOAL).0, "active");
    assert_eq!(h.count("outbox"), 1);
}
