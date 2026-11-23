use coglet_runtime::{MockEngine, Runtime};
use serde_json::{Value, json};

#[test]
fn every_primary_abi_case_is_executed() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let cases = cases.as_array().unwrap();
    assert_eq!(
        cases.len(),
        21,
        "update the corpus inventory when fixtures change"
    );
    let mut names = std::collections::BTreeSet::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert!(names.insert(name));
        run_case(name);
    }
}

fn run_case(name: &str) {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap();
    let mut runtime = Runtime::from_fixture(&case["steps"][0]["send"]).unwrap();
    let mut replay = runtime.clone();
    let mut actual = Value::Null;
    for step in case["steps"].as_array().unwrap() {
        if let Some(send) = step.get("send") {
            let mut engine = MockEngine::new(send["guest_script"].clone());
            actual = runtime.step(&send["inputs"], &mut engine).unwrap();
            assert!(
                serde_json::to_value(&runtime)
                    .unwrap()
                    .get("records")
                    .is_none(),
                "persisted state duplicates the growing journal history"
            );
        } else if let Some(expected) = step.get("expect") {
            assert_eq!(&actual, expected, "case: {name}");
        }
    }
    struct CaseProgram(Vec<Value>);
    impl coglet_runtime::Engine for CaseProgram {
        fn execute(
            &mut self,
            inputs: &Value,
            host: &mut coglet_runtime::StepHost,
        ) -> coglet_runtime::Result<coglet_runtime::EngineReport> {
            let script = self
                .0
                .get(inputs["step"].as_u64().ok_or("step")? as usize)
                .ok_or("missing independently supplied program")?
                .clone();
            coglet_runtime::Engine::execute(&mut MockEngine::new(script), inputs, host)
        }
    }
    let program = case["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("send"))
        .map(|s| s["guest_script"].clone())
        .collect();
    let hashes = replay
        .replay(runtime.records(), &mut CaseProgram(program))
        .unwrap();
    let expected: Vec<_> = runtime
        .records()
        .iter()
        .map(|r| r["output_hash"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(hashes, expected, "per-step replay state hashes: {name}");
}

#[test]
fn mock_engine_produces_no_effects_journal_from_inputs_not_expected_output() {
    run_case("no-effects");
}

#[test]
fn staged_emit_is_recorded_in_call_order() {
    run_case("action");
}

#[test]
fn timers_stage_with_runtime_logical_deadline() {
    run_case("timer");
}

#[test]
fn llm_request_is_staged_not_dispatched_as_surface_action() {
    run_case("llm");
}

#[test]
fn host_log_appends_canonical_jsonl() {
    run_case("log");
}

#[test]
fn successful_engine_report_cannot_bypass_budget_abort() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut send = cases[1]["steps"][0]["send"].clone();
    send["guest_script"]["usage"]["compute"] = json!(10001);
    send["guest_script"]["writes"] = json!({"/data/count":"3939"});
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    let record = runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    assert_eq!(record["journal"]["status"], "aborted");
    assert_eq!(record["journal"]["failure"]["kind"], "compute");
    assert_eq!(
        record["journal"]["input_hash"],
        record["journal"]["output_hash"]
    );
}

#[test]
fn authenticated_inbox_boundary_precedes_guest_input_hash() {
    run_case("inbox-boundary");
}

#[test]
fn llm_reply_requires_committed_pending_request() {
    run_case("async-llm-two-wakes");
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "async-llm-two-wakes")
        .unwrap();
    let sends: Vec<_> = case["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("send"))
        .collect();
    let mut runtime = Runtime::from_fixture(sends[0]).unwrap();
    runtime
        .step(
            &sends[0]["inputs"],
            &mut MockEngine::new(sends[0]["guest_script"].clone()),
        )
        .unwrap();
    let mut second = sends[1].clone();
    let change = &mut second["inputs"]["boundary"][0];
    let mut message: Value =
        serde_json::from_slice(&hex::decode(change["after"].as_str().unwrap()).unwrap()).unwrap();
    message["event"]["payload"]["request_id"] = json!("b".repeat(32));
    let mut line = serde_json::to_vec(&message).unwrap();
    line.push(b'\n');
    change["after"] = json!(hex::encode(line));
    let result = runtime.step(
        &second["inputs"],
        &mut MockEngine::new(second["guest_script"].clone()),
    );
    assert!(result.is_err(), "unknown LLM reply accepted");
}

#[test]
fn replay_reexecutes_engine_and_detects_changed_guest_program() {
    use coglet_runtime::{Engine, EngineReport, StepHost};
    struct Program(Vec<Value>);
    impl Engine for Program {
        fn execute(
            &mut self,
            inputs: &Value,
            host: &mut StepHost,
        ) -> coglet_runtime::Result<EngineReport> {
            let script = self
                .0
                .get(inputs["step"].as_u64().unwrap() as usize)
                .ok_or("missing recorded program step")?
                .clone();
            MockEngine::new(script).execute(inputs, host)
        }
    }
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "async-llm-two-wakes")
        .unwrap();
    let sends: Vec<_> = case["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("send"))
        .collect();
    let program: Vec<_> = sends.iter().map(|s| s["guest_script"].clone()).collect();
    let mut live = Runtime::from_fixture(sends[0]).unwrap();
    let mut engine = Program(program.clone());
    let mut expected_hashes = vec![];
    for send in &sends {
        expected_hashes.push(
            live.step(&send["inputs"], &mut engine).unwrap()["state_hash"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let records = live.records().to_vec();
    let mut replay = Runtime::from_fixture(sends[0]).unwrap();
    assert_eq!(
        replay
            .replay(&records, &mut Program(program.clone()))
            .unwrap(),
        expected_hashes
    );
    let mut changed_program = program.clone();
    changed_program[0]["writes"] = json!({"/data/count":"39"});
    let mut replay = Runtime::from_fixture(sends[0]).unwrap();
    assert!(
        replay
            .replay(&records, &mut Program(changed_program))
            .is_err()
    );
    assert_eq!(
        replay.replay(&records, &mut Program(program)).unwrap(),
        expected_hashes
    );
}

#[test]
fn scripted_abort_and_host_validation_regressions() {
    for name in [
        "debug-step",
        "abort-wall",
        "abort-compute",
        "abort-memory",
        "abort-host_bytes",
        "invalid-call",
        "duplicate-timer",
        "denied-action",
        "fresh-emit-id",
        "historical-emit-id",
        "fresh-timer-id",
        "historical-timer-id",
        "fresh-llm-id",
        "historical-llm-id",
    ] {
        run_case(name);
    }
}
