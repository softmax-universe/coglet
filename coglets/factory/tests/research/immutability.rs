use super::*;
#[test]
fn generic_annotations_cannot_mutate_research_or_immutable_records() {
    for op in [
        json!({"op":"EVALUATE","conclusion":"prose is not evidence"}),
        json!({"op":"REPLAN","reason":"annotation"}),
        json!({"op":"PRIORITIZE","priority":1}),
        json!({"op":"DEFER","until":100}),
    ] {
        let mut h = start();
        h.run(event(
            "factory.review",
            3,
            [("task_id", Value::Text(EXP.into()))],
        ));
        let original = h.get(EXP).1;
        propose(&mut h, 4, json!([op]));
        assert_eq!(h.get(EXP).1, original);
        h.run(event(
            "task.completed",
            5,
            [
                ("task_id", Value::Text(RUN_TASK.into())),
                ("note", Value::Text(report(Some(11)).to_string())),
            ],
        ));
        let evidence = h.get(EXP).1["evidence_id"].as_str().unwrap().to_string();
        propose(
            &mut h,
            6,
            json!([{"op":"EVALUATE","conclusion":"not learning"}]),
        );
        h.run(event(
            "factory.review",
            7,
            [("task_id", Value::Text(evidence.clone()))],
        ));
        let before = h.get(&evidence);
        propose(
            &mut h,
            8,
            json!([{"op":"REPLAN","reason":"tamper immutable evidence"}]),
        );
        assert_eq!(h.get(&evidence).1, before.1);
        h.run(event(
            "factory.review",
            9,
            [("task_id", Value::Text(EXP.into()))],
        ));
        propose(
            &mut h,
            10,
            json!([{"op":"LEARN","knowledge_id":"55555555555555555555555555555555"}]),
        );
        assert_eq!(h.get("55555555555555555555555555555555").0, "improvement");
    }
}
