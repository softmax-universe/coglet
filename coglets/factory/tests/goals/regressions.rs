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
fn cancelled_goal_task_does_not_accept_or_replan_late_model_reply() {
    let mut h = TestHost::new();
    authorize(&mut h);
    create(&mut h, 1, GOAL, 1);
    for seq in 10..13 {
        timer(&mut h, seq);
    }
    let task = h.get(GOAL).1["active_task"].as_str().unwrap().to_owned();
    h.run(event(
        "factory.review",
        20,
        [("task_id", Value::Text(task.clone()))],
    ));
    let reply = h.response(
        22,
        vec![Decision::CreateGoal {
            id: CHILD.into(),
            description: "must not revive cancelled work".into(),
            priority: 1,
            budget: 1,
        }],
    );
    let cancel = command_event(&mut h, 21, "factory.goal_cancel", json!({"goal_id":GOAL}));
    h.run(cancel);
    let requests = h.staged.len();
    h.run(reply);
    assert_eq!(h.staged.len(), requests, "cancelled owner must not replan");
    assert!(Repository::new(&mut h).get_object(CHILD).unwrap().is_none());
    assert_eq!(h.get(&task).0, "dispatched");
    assert_eq!(h.get(GOAL).0, "cancelled");
    h.run(event(
        "factory.review",
        23,
        [("task_id", Value::Text(task))],
    ));
    assert_eq!(
        h.staged.len(),
        requests,
        "explicit review cannot restart cancelled work"
    );
}

#[test]
fn cancellation_preserves_completed_dependency() {
    let mut h = TestHost::new();
    authorize(&mut h);
    create(&mut h, 1, GOAL, 1);
    for seq in 10..13 {
        timer(&mut h, seq);
    }
    let task = h.get(GOAL).1["active_task"].as_str().unwrap().to_owned();
    h.run(event(
        "task.completed",
        20,
        [("task_id", Value::Text(task))],
    ));
    let cancel = command_event(&mut h, 21, "factory.goal_cancel", json!({"goal_id":GOAL}));
    h.run(cancel);
    assert_eq!(h.get(GOAL).0, "completed");
    let wake = goal_event(
        &mut h,
        22,
        json!({"goal_id":CHILD,"description":"dependent","priority":1,"budget":1,"depends_on":[GOAL]}),
    );
    h.run(wake);
    for seq in 30..33 {
        timer(&mut h, seq);
    }
    assert_eq!(h.get(CHILD).0, "active");
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

// Drive the goal to `active` using only guest-emitted timers.
fn activate(h: &mut TestHost) -> String {
    authorize(h);
    create(h, 1, GOAL, 1);
    for seq in 10..13 {
        timer(h, seq);
    }
    assert_eq!(h.get(GOAL).0, "active");
    h.get(GOAL).1["active_task"].as_str().unwrap().to_owned()
}

fn drain_timers(h: &mut TestHost, first: u64) {
    let mut seq = first;
    while !h.timers.is_empty() {
        timer(h, seq);
        seq += 1;
        assert!(seq < first + 32, "review must settle");
    }
}

#[test]
fn model_defer_cannot_redispatch_active_goal_or_its_task() {
    for on_task in [false, true] {
        let mut h = TestHost::new();
        let task = activate(&mut h);
        let subject = if on_task { task.clone() } else { GOAL.into() };
        h.run(event(
            "factory.review",
            20,
            [("task_id", Value::Text(subject))],
        ));
        let reply = h.response(21, vec![Decision::Defer { until: 500 }]);
        h.run(reply);
        assert_eq!(h.get(GOAL).0, "active");
        assert_eq!(h.get(&task).0, "dispatched");
        assert!(h.get(GOAL).1.get("defer_until").is_none());
        assert!(h.get(&task).1.get("defer_until").is_none());
        // Run every emitted review, then one well past the refused deferral time.
        drain_timers(&mut h, 30);
        let mut late = event("factory.goal_review", 90, []);
        late.logical_time = 1_000;
        h.run(late);
        drain_timers(&mut h, 100);
        let goal = h.get(GOAL);
        assert_eq!(goal.0, "active");
        assert_eq!(goal.1["active_task"], task.as_str());
        assert_eq!(goal.1["attempts"], 1);
        assert_eq!(goal.1["remaining_budget"], 1);
        assert_eq!(h.count("outbox"), 1, "exactly one dispatch");
        assert_eq!(h.get("factory-policy").1["available_workers"], 1);
        // Cancellation still reaches the one and only attempt.
        let cancel = command_event(&mut h, 200, "factory.goal_cancel", json!({"goal_id":GOAL}));
        h.run(cancel);
        assert_eq!(h.get(GOAL).0, "cancelled");
        assert_eq!(h.get(&task).1["cancel_requested"], true);
        assert_eq!(h.count("outbox"), 2, "one dispatch plus one cancel");
    }
}

#[test]
fn model_defer_still_applies_to_unadmitted_goal() {
    let mut h = TestHost::new();
    authorize(&mut h);
    create(&mut h, 1, GOAL, 1);
    h.timers.clear();
    h.run(event(
        "factory.review",
        2,
        [("task_id", Value::Text(GOAL.into()))],
    ));
    let reply = h.response(3, vec![Decision::Defer { until: 500 }]);
    h.run(reply);
    assert_eq!(h.get(GOAL).0, "deferred");
    assert_eq!(h.get(GOAL).1["defer_until"], 500);
    assert_eq!(h.count("outbox"), 0);
}

#[test]
fn goal_task_lease_expiry_keeps_reservation_until_fenced_result() {
    let mut h = TestHost::new();
    let task = activate(&mut h);
    h.db.execute(
        "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'leased',0,'first',3,1,1)",
        [&task],
    )
    .unwrap();
    let mut expired = event(
        "task.lease_expired",
        20,
        [("task_id", Value::Text(task.clone()))],
    );
    expired.logical_time = 10;
    h.run(expired);
    assert_eq!(h.get(&task).0, "dispatched");
    assert_eq!(h.get(&task).1["capacity_reserved"], true);
    assert_eq!(h.get(GOAL).0, "active");
    assert_eq!(h.get("factory-policy").1["available_workers"], 1);
    assert!(h.staged.is_empty());
    // The host retries the same dispatch identity under a newer attempt.
    h.db.execute(
        "UPDATE work_queue SET lease_owner='second',lease_until=1000,attempt=2,version=2 WHERE task_id=?1",
        [&task],
    )
    .unwrap();
    let result = |lease: &str, attempt: i64, seq| {
        event(
            "task.completed",
            seq,
            [
                ("task_id", Value::Text(task.clone())),
                ("lease", Value::Text(lease.into())),
                ("attempt", Value::Integer(attempt.into())),
            ],
        )
    };
    h.run(result("first", 1, 21));
    assert_eq!(h.get(GOAL).0, "active", "expired attempt is stale");
    assert_eq!(h.get("factory-policy").1["available_workers"], 1);
    h.run(result("second", 2, 22));
    assert_eq!(h.get(&task).0, "completed");
    assert_eq!(h.get(GOAL).0, "completed");
    assert_eq!(h.get("factory-policy").1["available_workers"], 2);
    assert_eq!(h.count("outbox"), 1);
}
