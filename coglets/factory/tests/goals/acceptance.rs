//! Phase 3 acceptance (native executive path): budget/priority enforcement, no
//! duplicate goals from replay, deferral and cancellation. The actual-WAMR
//! counterparts live in crates/coglet-runtime/tests/factory_goals.
use super::*;

const A: &str = "a0000000000000000000000000000000";
const B: &str = "b0000000000000000000000000000000";
const C: &str = "c0000000000000000000000000000000";
const D: &str = "d0000000000000000000000000000000";
fn new_goal(h: &mut TestHost, seq: u64, id: &str, priority: u32, budget: u64, deps: &[&str]) {
    let wake = goal_event(
        h,
        seq,
        json!({"goal_id":id,"description":"goal","priority":priority,"budget":budget,"depends_on":deps}),
    );
    h.run(wake);
}
/// Drives the periodic review the way host timers do. Each step is one bounded
/// scan page or one admission; `from` is its strictly increasing logical time.
fn review(h: &mut TestHost, from: u64, steps: u64) {
    for seq in from..from + steps {
        h.run(event("factory.goal_review", seq, []));
    }
}
fn outbox_tasks(h: &TestHost) -> Vec<String> {
    let mut statement = h
        .db
        .prepare("SELECT payload_cbor_hex FROM outbox WHERE kind='factory.dispatch' ORDER BY rowid")
        .unwrap();
    let rows: Vec<String> = statement
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows.into_iter()
        .map(|hex| {
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
                .collect();
            let value = coglet_types::decode_profile(&bytes).unwrap();
            value.as_map().unwrap()[0].1.as_text().unwrap().to_owned()
        })
        .collect()
}
fn active(h: &TestHost, goal: &str) -> String {
    h.get(goal).1["active_task"].as_str().unwrap().to_owned()
}
fn finish(h: &mut TestHost, seq: u64, kind: &str, goal: &str) {
    let task = active(h, goal);
    h.run(event(kind, seq, [("task_id", Value::Text(task))]));
}

#[test]
fn goal_budget_and_priority_enforced() {
    let mut h = TestHost::new();
    authorize(&mut h);
    new_goal(&mut h, 1, A, 5, 1, &[]);
    new_goal(&mut h, 2, C, 9, 1, &[]);
    new_goal(&mut h, 3, B, 9, 1, &[]);
    // Highest priority, but blocked on a task that has not completed.
    new_goal(&mut h, 4, D, 100, 1, &[TASK]);
    review(&mut h, 100, 20);
    // Two workers: priority 9 first with deterministic id ties; never blocked D
    // nor lower-priority A while capacity is exhausted.
    let tasks = outbox_tasks(&h);
    assert_eq!(tasks, vec![active(&h, B), active(&h, C)]);
    assert_eq!(h.get(B).0, "active");
    assert_eq!(h.get(C).0, "active");
    assert_eq!(h.get(A).0, "open");
    assert_eq!(h.get(D).0, "open");
    assert_eq!(h.get(B).1["remaining_budget"], 0);
    let policy = h.get("factory-policy").1;
    assert_eq!(policy["remaining_budget"], 8);
    assert_eq!(policy["available_workers"], 0);
    assert!(h.staged.is_empty());
    // Capacity freed by a completion goes to A; D stays blocked.
    finish(&mut h, 200, "task.completed", B);
    assert_eq!(h.get(B).0, "completed");
    review(&mut h, 300, 20);
    assert_eq!(h.get(A).0, "active");
    assert_eq!(h.get(D).0, "open");
    assert_eq!(outbox_tasks(&h).len(), 3);
    // Once its dependency completes D outranks everything. A failure exhausts a
    // one-unit goal and neither reaches the model nor dispatches it again.
    h.db.execute("UPDATE objects SET status='completed' WHERE id=?1", [TASK])
        .unwrap();
    finish(&mut h, 400, "task.failed", A);
    assert_eq!(h.get(A).0, "exhausted");
    review(&mut h, 500, 20);
    assert_eq!(h.get(D).0, "active");
    assert_eq!(outbox_tasks(&h).len(), 4);
    review(&mut h, 600, 20);
    assert_eq!(outbox_tasks(&h).len(), 4);
    assert_eq!(h.get("factory-policy").1["remaining_budget"], 6);
    assert!(h.staged.is_empty());
}

#[test]
fn goal_replay_does_not_duplicate() {
    let mut h = TestHost::new();
    authorize(&mut h);
    new_goal(&mut h, 1, A, 5, 2, &[]);
    review(&mut h, 100, 6);
    let snapshot = (
        h.count("objects"),
        h.count("outbox"),
        h.count("events"),
        h.get(A),
    );
    assert_eq!(snapshot.1, 1);
    // Repeating every review event changes neither goals nor outbox effects.
    review(&mut h, 100, 6);
    assert_eq!(
        (
            h.count("objects"),
            h.count("outbox"),
            h.count("events"),
            h.get(A)
        ),
        snapshot
    );
    // The original goal event and a fresh-identity semantic repeat are no-ops.
    new_goal(&mut h, 1, A, 5, 2, &[]);
    assert_eq!((h.count("objects"), h.count("outbox")), (snapshot.0, 1));
    new_goal(&mut h, 2, A, 5, 2, &[]);
    assert_eq!((h.count("objects") - snapshot.0, h.count("outbox")), (1, 1)); // receipt only
    // An active goal is not admitted again by later reviews.
    review(&mut h, 200, 10);
    assert_eq!(h.count("outbox"), 1);
    assert_eq!(h.get(A).1["remaining_budget"], 1);
}

#[test]
fn goal_deferral_and_cancellation() {
    let mut h = TestHost::new();
    authorize(&mut h);
    new_goal(&mut h, 1, A, 5, 1, &[]);
    new_goal(&mut h, 2, B, 6, 1, &[]);
    new_goal(&mut h, 3, C, 7, 1, &[]);
    let defer = command_event(
        &mut h,
        4,
        "factory.goal_defer",
        json!({"goal_id":A,"until":1000}),
    );
    h.run(defer);
    let cancel = command_event(&mut h, 5, "factory.goal_cancel", json!({"goal_id":B}));
    h.run(cancel);
    assert_eq!(h.get(A).0, "deferred");
    assert_eq!(h.get(B).0, "cancelled");
    review(&mut h, 100, 12);
    // Only C ran: A is not yet due and cancelled B is never dispatched.
    assert_eq!(outbox_tasks(&h), vec![active(&h, C)]);
    assert_eq!(h.get(A).0, "deferred");
    // Cancelling a live goal records the exact attempt for the host's lease-fenced
    // cancel path; the attempt's result cannot revive the goal.
    let task = active(&h, C);
    h.db.execute(
        "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'leased',0,'tok',9999,1,1)",
        [&task],
    )
    .unwrap();
    let cancel = command_event(&mut h, 6, "factory.goal_cancel", json!({"goal_id":C}));
    h.run(cancel);
    h.run(event(
        "task.cancelled",
        7,
        [
            ("task_id", Value::Text(task.clone())),
            ("lease", Value::Text("tok".into())),
            ("attempt", Value::Integer(1.into())),
        ],
    ));
    assert_eq!(h.get(&task).0, "cancelled");
    assert_eq!(h.get(C).0, "cancelled");
    assert_eq!(h.get("factory-policy").1["available_workers"], 2);
    // A stale model decision for a goal cancelled after the request cannot revive it.
    new_goal(&mut h, 8, D, 3, 1, &[]);
    h.run(event(
        "factory.review",
        9,
        [("task_id", Value::Text(D.into()))],
    ));
    assert_eq!(h.staged.len(), 1);
    let cancel = command_event(&mut h, 10, "factory.goal_cancel", json!({"goal_id":D}));
    h.run(cancel);
    let reply = h.response(11, vec![Decision::Prioritize { priority: 50 }]);
    h.run(reply);
    assert_eq!(h.get(D).0, "cancelled");
    assert_eq!(h.get(D).1["priority"], 3);
    assert_eq!(h.staged.len(), 1);
    // After the deferral time A resumes (and only then).
    review(&mut h, 1001, 12);
    assert_eq!(h.get(A).0, "active");
    assert_eq!(h.get(B).0, "cancelled");
    assert_eq!(outbox_tasks(&h).len(), 2);
}
