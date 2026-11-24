//! Pure decision-component tests, NOT authenticated runtime/probation integration.
use coglet_hub::kernel::promotion::{Policy, Probation, ProbationDecision, ProbationSample};
use serde_json::json;

fn policy() -> Policy {
    Policy::parse(
        &json!({"version":"0.1","status":"DRAFT",
        "coglet":format!("coglet:{}", "b".repeat(64)),"revision":"1".repeat(32),
        "control_role":"application","human_approval":false,
        "gates":[{"kind":"journal_replay","suite":"r1","max_age_ms":100}],
        "probation":{"window_ms":1000,"health_interval_ms":10,"health_timeout_ms":100,
        "max_consecutive_failures":2,"max_step_failures":3,"max_budget_violations":2}}),
        "application",
    )
    .unwrap()
}

fn sample(at: u64, healthy: bool) -> ProbationSample {
    ProbationSample {
        coglet: policy().coglet().to_owned(),
        generation: 7,
        commit: "a".repeat(40),
        observed_at_ms: at,
        healthy,
        step_failures: 0,
        budget_violations: 0,
    }
}

#[test]
fn probation_component_requires_current_bound_health_before_expiry() {
    for bad in ["coglet", "generation", "commit", "future", "stale"] {
        let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
        let mut s = sample(109, true);
        match bad {
            "coglet" => s.coglet = format!("coglet:{}", "c".repeat(64)),
            "generation" => s.generation = 6,
            "commit" => s.commit = "b".repeat(40),
            "future" => s.observed_at_ms = 111,
            _ => s.observed_at_ms = 9,
        }
        assert_eq!(state.observe("boot1", 109, &s), ProbationDecision::Observe);
        assert_eq!(
            state.check("boot1", 110),
            ProbationDecision::Rollback,
            "{bad}"
        );
    }
    let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
    for at in (100..=1000).step_by(90) {
        assert_eq!(
            state.observe("boot1", at, &sample(at, true)),
            ProbationDecision::Observe
        );
    }
    assert_eq!(state.check("boot1", 1010), ProbationDecision::Stable);
    // Late health cannot rescue timeout, even at window expiry.
    let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
    assert_eq!(
        state.observe("boot1", 1010, &sample(1010, true)),
        ProbationDecision::Rollback
    );
}

#[test]
fn probation_component_thresholds_precede_completion_and_counters_never_reset() {
    for kind in ["consecutive", "step", "budget", "counter_regression"] {
        let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
        let mut first = sample(20, false);
        first.step_failures = 1;
        first.budget_violations = 1;
        assert_eq!(
            state.observe("boot1", 20, &first),
            ProbationDecision::Observe
        );
        let mut next = sample(30, kind != "consecutive");
        next.step_failures = 1;
        next.budget_violations = 1;
        match kind {
            "step" => next.step_failures = 3,
            "budget" => next.budget_violations = 2,
            "counter_regression" => next.step_failures = 0,
            _ => (),
        }
        assert_eq!(
            state.observe("boot1", 30, &next),
            ProbationDecision::Rollback,
            "{kind}"
        );
        assert_eq!(
            state.observe("boot1", 31, &sample(31, true)),
            ProbationDecision::Rollback
        );
    }
    for healthy_at_expiry in [true, false] {
        let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
        for at in (100..=1000).step_by(90) {
            assert_eq!(
                state.observe("boot1", at, &sample(at, true)),
                ProbationDecision::Observe
            );
        }
        let mut end = sample(1010, healthy_at_expiry);
        end.step_failures = 3;
        assert_eq!(
            state.observe("boot1", 1010, &end),
            ProbationDecision::Rollback
        );
    }
    // Duplicate unhealthy samples do not double count; healthy resets only the streak.
    let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
    for (at, healthy) in [(20, false), (20, false), (30, true), (40, false)] {
        let mut s = sample(at, healthy);
        s.step_failures = 2;
        s.budget_violations = 1;
        assert_eq!(state.observe("boot1", at, &s), ProbationDecision::Observe);
    }
    let mut end = sample(50, true);
    end.step_failures = 3;
    end.budget_violations = 1;
    assert_eq!(
        state.observe("boot1", 50, &end),
        ProbationDecision::Rollback
    );
}

#[test]
fn probation_component_timeout_is_exclusive_and_clock_loss_latches_rollback() {
    // Delayed observations cannot rewind an already observed supervisor instant.
    let mut late = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
    assert_eq!(late.check("boot1", 60), ProbationDecision::Observe);
    assert_eq!(
        late.observe("boot1", 70, &sample(50, true)),
        ProbationDecision::Observe
    );
    assert_eq!(late.check("boot1", 110), ProbationDecision::Rollback);
    let mut boundary = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
    assert_eq!(
        boundary.observe("boot1", 110, &sample(109, true)),
        ProbationDecision::Rollback
    );
    let start = u64::MAX - 1000;
    let mut high = Probation::start(&policy(), "boot1", start, 7, &"a".repeat(40)).unwrap();
    for delta in (90..=990).step_by(90) {
        let at = start + delta;
        assert_eq!(
            high.observe("boot1", at, &sample(at, true)),
            ProbationDecision::Observe
        );
    }
    assert_eq!(high.check("boot1", u64::MAX), ProbationDecision::Stable);
    let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
    assert_eq!(state.check("boot1", 109), ProbationDecision::Observe);
    assert_eq!(state.check("boot1", 110), ProbationDecision::Rollback);
    assert_eq!(state.check("boot1", 111), ProbationDecision::Rollback);
    for (epoch, now) in [("boot2", 11), ("boot1", 9), ("", 11)] {
        let mut state = Probation::start(&policy(), "boot1", 10, 7, &"a".repeat(40)).unwrap();
        assert_eq!(state.check(epoch, now), ProbationDecision::Rollback);
        assert_eq!(state.check("boot1", 12), ProbationDecision::Rollback);
    }
    assert!(Probation::start(&policy(), "", 10, 7, &"a".repeat(40)).is_err());
    assert!(Probation::start(&policy(), "boot1", u64::MAX, 7, &"a".repeat(40)).is_err());
    assert!(Probation::start(&policy(), "boot1", 10, 0, &"a".repeat(40)).is_err());
    assert!(Probation::start(&policy(), "boot1", 10, 7, "main").is_err());
}
