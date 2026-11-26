use super::*;
#[test]
fn malformed_or_unmatched_worker_evidence_never_becomes_knowledge() {
    for damage in [
        "baseline",
        "candidate",
        "inputs",
        "responses",
        "config",
        "evaluator",
        "budget",
        "cost",
        "trace",
        "artifact",
        "run",
        "experiment",
        "task",
        "missing",
        "unknown",
    ] {
        let mut h = start();
        let mut r = report(Some(11));
        match damage {
            "baseline" | "candidate" | "inputs" | "responses" | "config" | "evaluator" => {
                r["candidate"]["plan"][damage] = json!("9".repeat(64))
            }
            "budget" => r["candidate"]["plan"]["budget"] = json!(9),
            "cost" => r["candidate"]["cost"] = json!(11),
            "trace" | "artifact" => r["candidate"][damage] = json!("bad"),
            "run" => r["candidate"]["run_id"] = r["baseline"]["run_id"].clone(),
            "experiment" => r["experiment_id"] = json!(TASK),
            "task" => r["task_id"] = json!(TASK),
            "missing" => {
                r.as_object_mut().unwrap().remove("baseline");
            }
            _ => r["claimed_improvement"] = json!(true),
        }
        finish(&mut h, r);
        assert_eq!(h.get(EXP).1["comparison"], "inconclusive", "{damage}");
        propose(
            &mut h,
            4,
            json!([{"op":"LEARN","knowledge_id":"55555555555555555555555555555555"}]),
        );
        assert_eq!(
            h.db.query_row(
                "SELECT count(*) FROM objects WHERE type='knowledge'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
#[test]
fn negative_and_missing_scores_cannot_be_promoted_by_model() {
    for (score, expected) in [
        (Some(9), Some("regression")),
        (Some(10), Some("unchanged")),
        (None, None),
    ] {
        let mut h = start();
        finish(&mut h, report(score));
        propose(
            &mut h,
            4,
            json!([{"op":"LEARN","knowledge_id":"55555555555555555555555555555555"}]),
        );
        let n: i64 =
            h.db.query_row(
                "SELECT count(*) FROM objects WHERE type='knowledge' AND status='improvement'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        if let Some(status) = expected {
            assert_eq!(h.get("55555555555555555555555555555555").0, status);
            let kind: String =
                h.db.query_row(
                    "SELECT type FROM relations WHERE source_id=?1 AND target_id=?2",
                    ["55555555555555555555555555555555", HYP],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                kind,
                if status == "regression" {
                    "contradicts"
                } else {
                    "informs"
                }
            );
        }
    }
}
#[test]
fn experiment_dispatch_respects_global_admission_and_atomic_rollback() {
    for field in ["available_workers", "remaining_budget"] {
        let mut h = TestHost::new();
        Repository::new(&mut h)
            .create_object(&object(
                HYP,
                "hypothesis",
                "open",
                json!({"context_ids":[]}),
            ))
            .unwrap();
        let mut policy = h.get("factory-policy").1;
        policy[field] = json!(0);
        h.db.execute(
            "UPDATE objects SET data_json=?1 WHERE id='factory-policy'",
            [policy.to_string()],
        )
        .unwrap();
        h.run(event(
            "factory.review",
            1,
            [("task_id", Value::Text(HYP.into()))],
        ));
        propose(
            &mut h,
            2,
            json!([
                {"op":"EXPERIMENT","experiment_id":EXP,"task_id":RUN_TASK,"hypothesis_id":HYP,"plan":plan()},
                {"op":"DISPATCH","task_id":RUN_TASK}
            ]),
        );
        assert_eq!(h.count("outbox"), 0);
        assert_eq!(
            h.db.query_row(
                "SELECT count(*) FROM objects WHERE type='experiment'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
