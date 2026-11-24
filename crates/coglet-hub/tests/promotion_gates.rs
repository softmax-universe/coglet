use coglet_hub::kernel::promotion::{GateAuthority, GateBinding, GateReport, Policy};
use coglet_hub::{Error, Result};
use serde_json::{Value, json};
struct FixtureAuthority {
    trusted: Value,
}
impl GateAuthority for FixtureAuthority {
    fn verify(&self, report: &GateReport, binding: &GateBinding) -> Result<()> {
        if serde_json::to_value(report)? == self.trusted && binding.wasm_sha256 == "e".repeat(64) {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }
}
#[test]
fn gate_reports_require_exact_bindings_authentication_and_exclusive_freshness() {
    let revision = "1".repeat(32);
    let policy = Policy::parse(
        &json!({"version":"0.1","status":"DRAFT",
        "coglet":format!("coglet:{}", "b".repeat(64)), "revision":revision,
        "control_role":"application","human_approval":false,
        "gates":[{"kind":"journal_replay","suite":"replay-v1","max_age_ms":100}],
        "probation":{"window_ms":1000,"health_interval_ms":10,"health_timeout_ms":100,
            "max_consecutive_failures":2,"max_step_failures":3,"max_budget_violations":1}}),
        "application",
    )
    .unwrap();
    let binding = GateBinding {
        coglet: policy.coglet().into(),
        repo: "sandbox-pilot".into(),
        candidate: "c".repeat(40),
        base: Some("b".repeat(40)),
        policy_revision: revision.clone(),
        wasm_sha256: "e".repeat(64),
    };
    let report = json!({"kind":"journal_replay", "suite":"replay-v1", "candidate":binding.candidate,
        "base":binding.base,"policy_revision":revision,"checked_at_ms":100,
        "status":"passed","evidence_ref":"sha256:immutable-evidence"});
    let authority = FixtureAuthority {
        trusted: report.clone(),
    };
    assert!(
        policy
            .verify_reports(&json!([report]), &binding, 199, &authority)
            .is_ok()
    );
    assert!(
        policy
            .verify_reports(&json!([report]), &binding, 200, &authority)
            .is_err()
    );
    assert!(
        policy
            .verify_reports(&json!([report]), &binding, 99, &authority)
            .is_err()
    );
    assert!(
        policy
            .verify_reports(&json!([]), &binding, 199, &authority)
            .is_err()
    );
    assert!(
        policy
            .verify_reports(&json!([report, report]), &binding, 199, &authority)
            .is_err()
    );
    for (key, bad) in [
        ("candidate", json!("d".repeat(40))),
        ("base", json!(null)),
        ("policy_revision", json!("2".repeat(32))),
        ("suite", json!("other")),
        ("status", json!("failed")),
        ("evidence_ref", json!("forged")),
    ] {
        let mut altered = report.clone();
        altered[key] = bad;
        assert!(
            policy
                .verify_reports(&json!([altered]), &binding, 199, &authority)
                .is_err(),
            "{key}"
        );
    }
    let mut wrong_executable = binding.clone();
    wrong_executable.wasm_sha256 = "f".repeat(64);
    assert!(
        policy
            .verify_reports(&json!([report]), &wrong_executable, 199, &authority)
            .is_err()
    );
}
