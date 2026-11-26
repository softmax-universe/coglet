use super::*;

#[test]
fn goal_scan_revision_fences_inbox_insertion_at_cursor_free_boundaries() {
    for pages in [1, 2] {
        let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
        fx.grants.push("factory.dispatch".into());
        let engine = Wamr::interpreter().allow_db();
        let mut command = fx.event(100, "unused", "unused");
        command["kind"] = json!("factory.goal");
        command["payload"] = json!({"goal_id":GOAL,"description":"higher priority","priority":99,"budget":1,"depends_on":[]});
        let observer = fx.observer();
        observer.execute("INSERT INTO objects VALUES('factory-policy','policy','active',?1,1,10,10)",
            [json!({"model":"mock","remaining_budget":3,"available_workers":1,"max_priority":100}).to_string()]).unwrap();
        observer
            .execute(
                "INSERT INTO objects VALUES('factory-goal-authority','policy','active',?1,1,10,10)",
                [json!({"user_sources":[command["source"]],"worker_sources":[]}).to_string()],
            )
            .unwrap();
        let low = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        observer.execute("INSERT INTO objects VALUES(?1,'goal','open',?2,1,10,10)",
            [low, &json!({"description":"lower priority","priority":1,"remaining_budget":1,"context_ids":[]}).to_string()]).unwrap();
        drop(observer);
        let mut service = fx.service();
        let mut db = fx.db();
        let mut bootstrap = fx.event(1, "unused", "unused");
        bootstrap["kind"] = json!("factory.goal_review");
        bootstrap["payload"] = json!({});
        service.enqueue(bootstrap).unwrap();
        let mut step = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
        assert_eq!(step["journal"]["status"], "committed", "{step}");
        if pages == 2 {
            fx.now
                .set(step["journal"]["timers"][0]["at"].as_u64().unwrap());
            step = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
            assert_eq!(step["journal"]["status"], "committed", "{step}");
        }
        assert_eq!(fx.count("outbox"), 0);
        // Commit a genuine authenticated inbox boundary between exhausted
        // statuses or between the last page and admission. Reopen both hosts.
        drop(db);
        let message = json!({"index":0,"id":"55555555555555555555555555555555","sender":command["source"],"event":command});
        service.seal_boundary(&fx.setup["inputs"]["trace_id"], fx.now.get(), &[json!({
            "actor":format!("hub:{}","a".repeat(64)),"audit_id":"66666666666666666666666666666666",
            "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl",
            "before":null,"after":hex::encode(format!("{message}\n"))
        })]).unwrap();
        let segment = json!("77777777777777777777777777777777");
        service.start_segment(&segment).unwrap();
        fx.setup["inputs"]["trace_id"] = segment;
        drop(service);
        let (mut service, mut db) = (fx.service(), fx.db());
        let step = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
        assert_eq!(step["journal"]["status"], "committed", "{step}");
        assert_eq!(fx.object(GOAL).0, "open");
        // Existing and newly armed review wakes coincide; advance real timers,
        // never enqueue a fabricated review event to rescue a stopped scan.
        for _ in 0..3 {
            fx.now.set(fx.now.get() + 1);
            let step = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
            assert_eq!(step["journal"]["status"], "committed", "{step}");
        }
        assert_eq!(fx.object(GOAL).0, "active");
        assert_eq!(fx.object(low).0, "open");
        assert_eq!(fx.count("outbox"), 1);
    }
}
