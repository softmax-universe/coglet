//! Actual WASI guest, normal outbox host, durable gateway recordings and offline
//! replay. Receiver authentication is supplied by the existing embedding fixture.
use super::*;
mod backlog;
use coglet_factory_host::{ContractKanbanServer, FactoryHost};
const HYP: &str = "22222222222222222222222222222222";
const EXP: &str = "33333333333333333333333333333333";
const TASK: &str = "44444444444444444444444444444444";
const KNOW: &str = "55555555555555555555555555555555";
fn plan() -> Value {
    json!({"baseline":"a".repeat(64),"candidate":"b".repeat(64),"inputs":"c".repeat(64),"responses":"d".repeat(64),"config":"e".repeat(64),"evaluator":"f".repeat(64),"budget":10})
}
fn report(score: Option<i64>) -> Value {
    let run = |c: char, code: &str, s: Option<i64>| json!({"run_id":c.to_string().repeat(32),"code":code,"plan":plan(),"trace":c.to_string().repeat(64),"artifact":"9".repeat(64),"cost":5,"score":s});
    json!({"experiment_id":EXP,"task_id":TASK,"baseline":run('1',&"a".repeat(64),Some(10)),"candidate":run('2',&"b".repeat(64),score)})
}
fn data(fx: &Fx, key: &str) -> Value {
    let text: String = fx
        .observer()
        .query_row("SELECT data_json FROM objects WHERE id=?1", [key], |r| {
            r.get(0)
        })
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
struct Research {
    fx: Fx,
    engine: Wamr,
    inbox: String,
    index: u64,
    segments: Vec<(Value, Value)>,
}
impl Research {
    fn new() -> Self {
        let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
        fx.grants
            .extend(["llm.complete".into(), "factory.dispatch".into()]);
        let policy =
            json!({"model":"mock","remaining_budget":9,"available_workers":2,"max_priority":100});
        fx.observer()
            .execute(
                "INSERT INTO objects VALUES('factory-policy','policy','active',?1,1,10,10)",
                [policy.to_string()],
            )
            .unwrap();
        fx.observer()
            .execute(
                "INSERT INTO objects VALUES(?1,'hypothesis','open',?2,1,10,10)",
                [
                    HYP,
                    json!({"description":"candidate improves baseline","context_ids":[]})
                        .to_string()
                        .as_str(),
                ],
            )
            .unwrap();
        Self {
            fx,
            engine: Wamr::interpreter().allow_db(),
            inbox: String::new(),
            index: 0,
            segments: vec![],
        }
    }
    fn step(&mut self) -> Value {
        let (mut service, mut db) = (self.fx.service(), self.fx.db());
        if self.index > 0 {
            let initial = service
                .export_trace(&self.fx.setup["inputs"]["trace_id"], self.fx.now.get())
                .unwrap();
            self.segments
                .push((self.fx.setup["inputs"]["trace_id"].clone(), initial));
        }
        let step = self
            .fx
            .step(&mut service, &mut db, &self.engine)
            .unwrap()
            .unwrap();
        assert_eq!(step["journal"]["status"], "committed", "{step}");
        step
    }
    fn deliver(&mut self, kind: &str, payload: Value) -> Value {
        let mut e = self.fx.event(100 + self.index, "unused", "unused");
        e["kind"] = json!(kind);
        e["payload"] = payload;
        if kind == "llm.reply" {
            e["correlation_id"] = e["payload"]["request_id"].clone();
        }
        let message = json!({"index":self.index,"id":format!("{:032x}",1000+self.index),"sender":e["source"],"event":e});
        let before = if self.inbox.is_empty() {
            Value::Null
        } else {
            json!(hex::encode(self.inbox.as_bytes()))
        };
        self.inbox.push_str(&format!("{message}\n"));
        let mut service = self.fx.service();
        service.seal_boundary(&self.fx.setup["inputs"]["trace_id"],self.fx.now.get(),&[json!({
            "actor":format!("hub:{}","a".repeat(64)),"audit_id":format!("{:032x}",2000+self.index),
            "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl","before":before,"after":hex::encode(self.inbox.as_bytes())
        })]).unwrap();
        let segment = json!(format!("{:032x}", 3000 + self.index));
        service.start_segment(&segment).unwrap();
        self.fx.setup["inputs"]["trace_id"] = segment;
        self.index += 1;
        drop(service); // reopen after each authenticated boundary
        self.step()
    }
    fn reply(&mut self, request: &Value, operations: Value) -> Value {
        use coglet_llm::{Admission, Completion, Gateway, Provider, Route};
        use coglet_mock::llm::{MockLlm, Script};
        let routes = || {
            [(
                "mock".into(),
                Route {
                    provider: "scripted".into(),
                    ceiling: 2,
                },
            )]
            .into()
        };
        let path = self.fx.root.path().join("research-gateway.sqlite");
        let mut g = Gateway::open(path.to_str().unwrap(), routes()).unwrap();
        g.set_budget(C, 100).unwrap();
        g.enqueue_at(
            Admission {
                coglet: C,
                llm_complete_granted: true,
            },
            request,
            1,
        )
        .unwrap();
        drop(g);
        let mut g = Gateway::open(path.to_str().unwrap(), routes()).unwrap();
        let prompt: Value =
            serde_json::from_str(request["input"]["prompt"].as_str().unwrap()).unwrap();
        let proposal = json!({"state_version":prompt["state_version"],"operations":operations});
        let mut provider = MockLlm::new([Script {
            provider: "scripted".into(),
            request: request.clone(),
            result: Ok(Completion {
                cost: 1,
                reply: json!({"request_id":request["id"],"status":"succeeded","output":{"text":proposal.to_string()}}),
            }),
        }]);
        g.dispatch_next_checked(|_| Ok(()), |call| provider.complete(call))
            .unwrap();
        assert_eq!(provider.calls.len(), 1);
        drop(g);
        let g = Gateway::open(path.to_str().unwrap(), routes()).unwrap();
        let payload = coglet_types::decode(
            "abi::llm-reply",
            &g.replay(C, request["id"].as_str().unwrap()).unwrap(),
        )
        .unwrap()
        .deserialized()
        .unwrap();
        drop(g);
        self.deliver("llm.reply", payload)
    }
    fn start(&mut self) {
        let mut e = self.fx.event(1, "unused", "unused");
        e["kind"] = json!("factory.review");
        e["payload"] = json!({"task_id":HYP});
        self.fx.service().enqueue(e).unwrap();
        let step = self.step();
        let req = step["journal"]["llm_requests"][0].clone();
        self.reply(&req,json!([
            {"op":"EXPERIMENT","experiment_id":EXP,"task_id":TASK,"hypothesis_id":HYP,"plan":plan()},
            {"op":"DISPATCH","task_id":TASK}
        ]));
        assert_eq!(self.fx.object(EXP).0, "running");
        assert_eq!(self.fx.object(TASK).0, "dispatched");
    }
    fn outcome(&mut self, success: bool, note: &str) -> Value {
        let server = ContractKanbanServer::spawn();
        let mut db = self.fx.db();
        let mut host = FactoryHost::from_adapter(server.adapter());
        host.run(&mut db, 20).unwrap();
        let intent = db.outbox_in_state("dispatching").unwrap();
        assert_eq!(intent.len(), 1);
        let binding = db.get_binding(&intent[0].intent_id).unwrap().unwrap();
        server.post_result(&binding, success, note);
        let result = host.run(&mut db, 21).unwrap();
        assert_eq!(result.follow_ups.len(), 1);
        let follow = result.follow_ups[0].clone();
        drop((db, host));
        let mut db = self.fx.db();
        let mut host = FactoryHost::from_adapter(server.adapter());
        assert_eq!(host.run(&mut db, 22).unwrap().follow_ups, result.follow_ups);
        drop((db, host));
        let step=self.deliver(&follow.kind,json!({"task_id":follow.task_id,"attempt":follow.attempt,"lease":follow.lease,"note":follow.note}));
        let mut db = self.fx.db();
        let mut host = FactoryHost::from_adapter(server.adapter());
        host.dispatcher()
            .acknowledge(&mut db, &follow.event_id)
            .unwrap();
        step
    }
    fn offline(self) {
        let service = self.fx.service();
        let frames = self.fx.journal(&service);
        drop(service);
        std::fs::remove_file(&self.fx.db_path).unwrap();
        std::fs::remove_file(self.fx.root.path().join("research-gateway.sqlite")).unwrap();
        assert!(!self.segments.is_empty());
        for (trace, initial) in self.segments {
            let refs: Vec<&[u8]> = frames
                .iter()
                .filter(|f| {
                    let r = coglet_types::decode_profile(f).unwrap();
                    field(&r, "trace_id").as_text() == trace.as_str()
                        && r.as_map()
                            .unwrap()
                            .iter()
                            .any(|(k, _)| k.as_text() == Some("step"))
                })
                .map(Vec::as_slice)
                .collect();
            assert_eq!(refs.len(), 1);
            coglet_runtime::Runtime::import_trace(&initial)
                .unwrap()
                .replay_database(&refs, &self.engine, 32 * 1024 * 1024, 65536)
                .unwrap();
        }
        assert!(!self.fx.db_path.exists());
    }
}
#[test]
fn failed_experiment_triggers_investigation() {
    let mut r = Research::new();
    r.start();
    let step = r.outcome(false, "experiment failed");
    assert_eq!(r.fx.object(EXP).0, "failed");
    r.reply(&step["journal"]["llm_requests"][0],json!([
        {"op":"INVESTIGATE","task_id":"66666666666666666666666666666666","hypothesis_id":"77777777777777777777777777777777","description":"diagnose failed comparison"},
        {"op":"DISPATCH","task_id":"66666666666666666666666666666666"}
    ]));
    assert_eq!(r.fx.count("outbox"), 2);
    assert_eq!(r.fx.object("77777777777777777777777777777777").0, "open");
    let count = r.fx.count("objects");
    r.deliver(
        "task.failed",
        json!({"task_id":TASK,"attempt":1,"lease":"old","note":"duplicate terminal"}),
    );
    assert_eq!(r.fx.count("outbox"), 2);
    assert_eq!(r.fx.count("objects"), count + 1); // receipt only
    r.offline();
}
fn success(score: Option<i64>, tamper: bool) -> Research {
    let mut r = Research::new();
    r.start();
    let step = r.outcome(true, &report(score).to_string());
    let evidence = data(&r.fx, EXP)["evidence_id"].as_str().unwrap().to_owned();
    let original = data(&r.fx, &evidence);
    if tamper {
        r.fx.observer()
            .execute("UPDATE objects SET data_json='{}' WHERE id=?1", [&evidence])
            .unwrap();
    }
    r.reply(
        &step["journal"]["llm_requests"][0],
        json!([{"op":"LEARN","knowledge_id":KNOW}]),
    );
    if tamper || score.is_none() {
        assert_eq!(
            r.fx.observer()
                .query_row(
                    "SELECT count(*) FROM objects WHERE type='knowledge'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    } else {
        assert_eq!(data(&r.fx, KNOW)["evidence"], original);
        assert_eq!(original["report"], report(score));
    }
    r
}
#[test]
fn evidence_provenance_is_preserved_and_validated() {
    success(Some(11), false).offline();
    success(Some(11), true).offline();
}
#[test]
fn candidate_baseline_comparison_is_reproducible() {
    for (score, expected) in [
        (Some(11), "improvement"),
        (Some(9), "regression"),
        (Some(10), "unchanged"),
        (None, "inconclusive"),
    ] {
        let a = success(score, false);
        let b = success(score, false);
        assert_eq!(data(&a.fx, EXP)["comparison"], expected);
        let eid = data(&a.fx, EXP)["evidence_id"].as_str().unwrap().to_owned();
        assert_eq!(data(&a.fx, &eid), data(&b.fx, &eid));
        a.offline();
        b.offline();
    }
}
#[test]
fn research_replay_uses_recorded_responses() {
    success(Some(11), false).offline();
}
