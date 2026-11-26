use super::*;
fn remember(times: &mut std::collections::BTreeSet<u64>, step: &Value) {
    for t in step["journal"]["timers"].as_array().unwrap() {
        times.insert(t["at"].as_u64().unwrap());
    }
}
fn request_from_timers(r: &mut Research, times: &mut std::collections::BTreeSet<u64>) -> Value {
    for _ in 0..24 {
        let at = *times.first().expect("backlog must have an emitted wake");
        r.fx.now.set(at);
        let (mut service, mut db) = (r.fx.service(), r.fx.db());
        let Some(step) = r.fx.step(&mut service, &mut db, &r.engine).unwrap() else {
            times.remove(&at);
            continue;
        };
        assert_eq!(step["journal"]["status"], "committed", "{step}");
        remember(times, &step);
        if !step["journal"]["llm_requests"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            return step;
        }
    }
    panic!("backlog did not drain through emitted timers")
}
#[test]
fn two_research_completions_drain_via_revision_fenced_read_only_selection() {
    let mut r = Research::new();
    const EXP2: &str = "66666666666666666666666666666666";
    const TASK2: &str = "77777777777777777777777777777777";
    // Already-running worker fixtures; normal public dispatch/result integration
    // is independently exercised by the four Phase 4 acceptance scenarios.
    for (e, t) in [(EXP, TASK), (EXP2, TASK2)] {
        r.fx.observer().execute("INSERT INTO objects VALUES(?1,'experiment','running',?2,1,10,10)",[e,json!({"hypothesis_id":HYP,"task_id":t,"plan":plan(),"context_ids":[HYP],"evidence_id":null,"evidence_hash":null,"comparison":null}).to_string().as_str()]).unwrap();
        r.fx.observer()
            .execute(
                "INSERT INTO objects VALUES(?1,'task','dispatched',?2,1,10,10)",
                [
                    t,
                    json!({"experiment_id":e,"context_ids":[],"capacity_reserved":false})
                        .to_string()
                        .as_str(),
                ],
            )
            .unwrap();
    }
    let mut e = r.fx.event(1, "unused", "unused");
    e["kind"] = json!("factory.review");
    e["payload"] = json!({"task_id":"T1"});
    r.fx.service().enqueue(e).unwrap();
    let pending = r.step();
    let request = pending["journal"]["llm_requests"][0].clone();
    let mut times = std::collections::BTreeSet::new();
    remember(&mut times, &pending);
    let first = r.deliver(
        "task.completed",
        json!({"task_id":TASK,"note":report(Some(11)).to_string()}),
    );
    remember(&mut times, &first);
    let mut other = report(Some(9));
    other["experiment_id"] = json!(EXP2);
    other["task_id"] = json!(TASK2);
    let second = r.deliver(
        "task.completed",
        json!({"task_id":TASK2,"note":other.to_string()}),
    );
    remember(&mut times, &second);
    assert_eq!(
        r.fx.observer()
            .query_row(
                "SELECT count(*) FROM objects WHERE type='deliberation' AND status='pending'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    let cleared = r.reply(
        &request,
        json!([{"op":"REPLAN","reason":"finish unrelated request"}]),
    );
    remember(&mut times, &cleared);
    let first = request_from_timers(&mut r, &mut times);
    let prompt: Value = serde_json::from_str(
        first["journal"]["llm_requests"][0]["input"]["prompt"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let first_id = prompt["context"]["subject"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!([EXP, EXP2].contains(&first_id.as_str()));
    let replied = r.reply(
        &first["journal"]["llm_requests"][0],
        json!([{"op":"LEARN","knowledge_id":KNOW}]),
    );
    remember(&mut times, &replied);
    let second = request_from_timers(&mut r, &mut times);
    let prompt: Value = serde_json::from_str(
        second["journal"]["llm_requests"][0]["input"]["prompt"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_ne!(prompt["context"]["subject"]["id"], first_id);
    r.reply(
        &second["journal"]["llm_requests"][0],
        json!([{"op":"LEARN","knowledge_id":"88888888888888888888888888888888"}]),
    );
    assert_eq!(
        r.fx.observer()
            .query_row(
                "SELECT count(*) FROM objects WHERE type='knowledge'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(r.fx.object(EXP).0, "concluded");
    assert_eq!(r.fx.object(EXP2).0, "concluded");
    // At least one real paging read committed without changing the DB revision,
    // and its transactional selection file survived reopening the runtime.
    let service = r.fx.service();
    let frames = r.fx.journal(&service);
    assert!(frames.iter().any(|f| {
        let v: Value = coglet_types::decode_profile(f)
            .unwrap()
            .deserialized()
            .unwrap();
        v.get("step").is_some()
            && v["db_before"] == v["db_after"]
            && v["output_files"]
                .to_string()
                .contains("deliberation-pick.json")
    }));
}
