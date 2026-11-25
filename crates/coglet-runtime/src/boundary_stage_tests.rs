use super::*;

fn case(name: &str) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["steps"][0]["send"]
        .clone()
}

/// The pre-optimization behavior: clone the whole runtime, then apply.
fn reference(runtime: &Runtime, changes: &[Value]) -> Result<StagedBoundaryInputs> {
    let mut staged = runtime.clone();
    staged.apply_boundary_changes(changes)?;
    Ok((
        staged.files,
        staged.inbox_wakes,
        staged.pending_llm,
        staged.pending_actions,
        staged.next_source_seq,
    ))
}

#[test]
fn empty_boundary_matches_full_clone_path_and_leaves_runtime_untouched() {
    for name in ["no-effects", "async-llm-two-wakes", "action", "llm"] {
        let send = case(name);
        let runtime = Runtime::from_fixture(&send).unwrap();
        let before = serde_json::to_value(&runtime).unwrap();
        assert_eq!(
            runtime.stage_boundary(&[]).unwrap(),
            reference(&runtime, &[]).unwrap(),
            "{name}"
        );
        assert_eq!(serde_json::to_value(&runtime).unwrap(), before, "{name}");
    }
}

#[test]
fn non_empty_boundary_still_applies_and_matches_full_clone_path() {
    let send = case("inbox-boundary");
    let runtime = Runtime::from_fixture(&send).unwrap();
    let changes = send["inputs"]["boundary"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    let staged = runtime.stage_boundary(changes).unwrap();
    assert_eq!(staged, reference(&runtime, changes).unwrap());
    // The authenticated inbox append is really visible, not skipped by the
    // empty fast path, and the source runtime is not mutated.
    assert_ne!(staged.0, runtime.files);
    assert_eq!(
        crate::boundary::inbox_with_profile(&staged.0, runtime.contract_profile)
            .unwrap()
            .len(),
        crate::boundary::inbox_with_profile(&runtime.files, runtime.contract_profile)
            .unwrap()
            .len()
            + 1
    );
}

#[test]
fn invalid_boundary_still_fails_and_is_not_bypassed_by_fast_path() {
    let send = case("inbox-boundary");
    let runtime = Runtime::from_fixture(&send).unwrap();
    let mut change = send["inputs"]["boundary"][0].clone();
    change["before"] = Value::String("00".into());
    let before = serde_json::to_value(&runtime).unwrap();
    assert_eq!(
        runtime.stage_boundary(std::slice::from_ref(&change)),
        Err("boundary before bytes mismatch".to_string())
    );
    change["target"] = Value::String("coglet:wrong".into());
    assert!(
        runtime
            .stage_boundary(std::slice::from_ref(&change))
            .is_err()
    );
    assert_eq!(serde_json::to_value(&runtime).unwrap(), before);
}
