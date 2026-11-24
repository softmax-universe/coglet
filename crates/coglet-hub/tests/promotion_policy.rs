//! Component tests only. These do not establish runtime activation qualification.
use coglet_hub::kernel::promotion::Policy;
use serde_json::{Value, json};

fn policy() -> Value {
    json!({"version":"0.1", "status":"DRAFT", "coglet":format!("coglet:{}", "b".repeat(64)),
        "revision":"11111111111111111111111111111111", "control_role":"application", "human_approval":false,
        "gates":[{"kind":"journal_replay","suite":"replay-v1","max_age_ms":100}],
        "probation":{"window_ms":1000,"health_interval_ms":10,"health_timeout_ms":100,
            "max_consecutive_failures":2,"max_step_failures":3,"max_budget_violations":1}})
}

#[test]
fn policy_requires_replay_unique_gates_and_enrolled_role() {
    let valid = policy();
    assert!(Policy::parse(&valid, "application").is_ok());
    assert!(Policy::parse(&valid, "factory").is_err());
    let mut no_replay = valid.clone();
    no_replay["gates"][0]["kind"] = json!("headless_game");
    assert!(Policy::parse(&no_replay, "application").is_err());
    let mut duplicate = valid.clone();
    duplicate["gates"]
        .as_array_mut()
        .unwrap()
        .push(valid["gates"][0].clone());
    assert!(Policy::parse(&duplicate, "application").is_err());
    let mut control = valid.clone();
    control["control_role"] = json!("factory");
    assert!(Policy::parse(&control, "factory").is_err());
    control["human_approval"] = json!(true);
    assert!(Policy::parse(&control, "factory").is_ok());
}

#[test]
fn policy_rejects_closed_shape_timing_and_lexical_violations() {
    let valid = policy();
    for path in [
        "window_ms",
        "health_interval_ms",
        "health_timeout_ms",
        "max_consecutive_failures",
        "max_step_failures",
        "max_budget_violations",
    ] {
        let mut value = valid.clone();
        value["probation"][path] = json!(0);
        assert!(Policy::parse(&value, "application").is_err(), "zero {path}");
    }
    for (pointer, bad) in [
        ("/kernel_authority", json!(true)),
        ("/probation/future", json!(1)),
        ("/gates/0/extra", json!(1)),
        ("/gates/0/max_age_ms", json!(true)),
        ("/gates/0/max_age_ms", json!(0)),
        ("/gates/0/suite", json!("")),
        ("/revision", json!("1".repeat(32) + "\n")),
        (
            "/coglet",
            json!("coglet:".to_owned() + &"b".repeat(64) + "\n"),
        ),
        ("/probation/health_timeout_ms", json!(1001)),
        ("/probation/health_interval_ms", json!(101)),
    ] {
        let mut value = valid.clone();
        // Existing-field mutations use JSON pointers; closed-field controls are explicit.
        if pointer == "/kernel_authority" {
            value["kernel_authority"] = bad;
        } else if pointer == "/probation/future" {
            value["probation"]["future"] = bad;
        } else if pointer == "/gates/0/extra" {
            value["gates"][0]["extra"] = bad;
        } else {
            *value.pointer_mut(pointer).unwrap() = bad;
        }
        assert!(
            Policy::parse(&value, "application").is_err(),
            "{pointer}: {value}"
        );
    }
}
