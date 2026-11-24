use coglet_llm::{Admission, Completion, Gateway, Route};
use coglet_runtime::{Engine, EngineReport, MockEngine, Runtime, StepHost};
use serde_json::{Value, json};
use std::collections::BTreeMap;
struct Program(Vec<Value>);
impl Engine for Program {
    fn execute(
        &mut self,
        inputs: &Value,
        host: &mut StepHost,
    ) -> coglet_runtime::Result<EngineReport> {
        MockEngine::new(self.0[inputs["step"].as_u64().unwrap() as usize].clone())
            .execute(inputs, host)
    }
}
#[test]
fn recorded_call_replays_same_runtime_state_hash_without_provider() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "async-llm-two-wakes")
        .unwrap();
    let sends: Vec<Value> = case["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("send").cloned())
        .collect();
    let program: Vec<_> = sends.iter().map(|s| s["guest_script"].clone()).collect();
    let mut live = Runtime::from_fixture(&sends[0]).unwrap();
    let mut initial = live.clone();
    let first = live
        .step(&sends[0]["inputs"], &mut Program(program.clone()))
        .unwrap();
    let request = &first["journal"]["llm_requests"][0];
    let c = sends[0]["inputs"]["coglet"].as_str().unwrap();
    let id = request["id"].as_str().unwrap();
    let mut g = Gateway::open(
        ":memory:",
        BTreeMap::from([(
            "small".into(),
            Route {
                provider: "local".into(),
                ceiling: 10,
            },
        )]),
    )
    .unwrap();
    g.set_budget(c, 10).unwrap();
    g.enqueue(
        Admission {
            coglet: c,
            llm_complete_granted: true,
        },
        request,
    )
    .unwrap();
    g.dispatch_next(|_| {
        Ok(Completion {
            reply: json!({"request_id":id,"status":"succeeded","output":{"text":"hello"}}),
            cost: 7,
        })
    })
    .unwrap();
    let reply: Value = coglet_types::decode_typed("llm-reply", &g.replay(c, id).unwrap()).unwrap();
    let mut second = sends[1]["inputs"].clone();
    let mut inbox: Value = serde_json::from_slice(
        &hex::decode(second["boundary"][0]["after"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    inbox["event"]["payload"] = reply;
    let mut line = serde_json::to_vec(&inbox).unwrap();
    line.push(b'\n');
    g.record_inbox(c, id, &line).unwrap();
    second["boundary"][0]["after"] = json!(hex::encode(g.replay_inbox(c, id).unwrap()));
    let second_result = live.step(&second, &mut Program(program.clone())).unwrap();
    let expected = vec![
        first["state_hash"].as_str().unwrap().to_owned(),
        second_result["state_hash"].as_str().unwrap().to_owned(),
    ];
    assert_ne!(
        expected[0], expected[1],
        "inbox reply must affect the real runtime tree hash"
    );
    assert!(
        !g.dispatch_next(|_| panic!("offline replay dispatched"))
            .unwrap()
    );
    assert_eq!(
        initial
            .replay(live.records(), &mut Program(program))
            .unwrap(),
        expected
    );
    let mut tampered = live.records().to_vec();
    tampered[1]["boundary"][0]["after"] = json!("00");
    let mut fresh = Runtime::from_fixture(&sends[0]).unwrap();
    assert!(
        fresh
            .replay(
                &tampered,
                &mut Program(sends.iter().map(|s| s["guest_script"].clone()).collect())
            )
            .is_err()
    );
}
