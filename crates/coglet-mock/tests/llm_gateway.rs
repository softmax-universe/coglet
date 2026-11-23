use coglet_llm::{Admission, Completion, Gateway, Provider, Route, failure};
use coglet_mock::llm::{MockLlm, Script};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Store(PathBuf);
impl Store {
    fn new() -> Self {
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = root.join(format!(
            "coglet-llm-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> Gateway {
        Gateway::open(self.0.join("ledger.db"), routes()).unwrap()
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn routes() -> BTreeMap<String, Route> {
    BTreeMap::from([(
        "small".into(),
        Route {
            provider: "scripted".into(),
            ceiling: 10,
        },
    )])
}
fn coglet(n: char) -> String {
    format!("coglet:{}", n.to_string().repeat(64))
}
fn request(n: u64) -> Value {
    json!({"id":format!("{n:032x}"),"correlation_id":"c".repeat(32),"capability":"llm.complete","model":"small","input":{"prompt":"hello"}})
}
fn admission(coglet: &str) -> Admission<'_> {
    Admission {
        coglet,
        llm_complete_granted: true,
    }
}
fn success(req: &Value, cost: i64) -> Completion {
    Completion {
        reply: json!({"request_id":req["id"],"status":"succeeded","output":{"text":"héllo\nworld","tokens":u64::MAX}}),
        cost,
    }
}
fn scripted(req: &Value, cost: i64) -> Script {
    Script {
        provider: "scripted".into(),
        request: req.clone(),
        result: Ok(success(req, cost)),
    }
}

#[test]
fn recorded_reply_and_exact_inbox_replay_offline_after_reopen() {
    let store = Store::new();
    let c = coglet('a');
    let req = request(1);
    let id = req["id"].as_str().unwrap();
    let mut gateway = store.open();
    gateway.set_budget(&c, 20).unwrap();
    gateway.enqueue(admission(&c), &req).unwrap();
    let mut mock = MockLlm::new([scripted(&req, 7)]);
    assert!(gateway.dispatch_next(|call| mock.complete(call)).unwrap());
    let raw = gateway.replay(&c, id).unwrap();
    let reply = success(&req, 7).reply;
    let msg = json!({"index":0,"id":"d".repeat(32),"sender":format!("hub:{}","b".repeat(64)),"event":{"source":format!("hub:{}","b".repeat(64)),"seq":u64::MAX,"logical_time":42,"kind":"llm.reply","payload":reply,"correlation_id":req["correlation_id"]}});
    let mut line = serde_json::to_vec(&msg).unwrap();
    line.push(b'\n');
    gateway.record_inbox(&c, id, &line).unwrap();
    // Changed binding or any noncanonical spelling cannot replace the recording.
    let mut wrong = msg.clone();
    wrong["event"]["correlation_id"] = json!("e".repeat(32));
    let mut wrong_line = serde_json::to_vec(&wrong).unwrap();
    wrong_line.push(b'\n');
    assert!(gateway.record_inbox(&c,id,&wrong_line).is_err());
    let mut spaced = vec![b' ']; spaced.extend(&line);
    assert!(gateway.record_inbox(&c,id,&spaced).is_err());
    wrong = msg.clone(); wrong["index"] = json!(1);
    let mut changed = serde_json::to_vec(&wrong).unwrap(); changed.push(b'\n');
    assert!(gateway.record_inbox(&c,id,&changed).is_err());
    gateway.record_inbox(&c, id, &line).unwrap();
    drop(gateway);
    let mut gateway = store.open();
    assert_eq!(gateway.replay(&c, id).unwrap(), raw);
    assert_eq!(gateway.replay_inbox(&c, id).unwrap(), line);
    assert!(
        !gateway
            .dispatch_next(|_| panic!("offline replay called provider"))
            .unwrap()
    );
    assert!(gateway.replay(&c, &"f".repeat(32)).is_err());
    assert_eq!(mock.calls.len(), 1);
    assert_eq!(mock.remaining(), 0);
}

#[test]
fn reservations_prevent_overspend_and_ledger_sums_per_coglet() {
    let store = Store::new();
    let mut g = store.open();
    let a = coglet('a');
    let b = coglet('b');
    g.set_budget(&a, 20).unwrap();
    g.set_budget(&b, 10).unwrap();
    g.enqueue(admission(&a), &request(1)).unwrap();
    g.enqueue(admission(&a), &request(2)).unwrap();
    assert_eq!(
        g.enqueue(admission(&a), &request(3)).unwrap_err(),
        "resource_exhausted"
    );
    g.enqueue(admission(&b), &request(1)).unwrap();
    let mut mock = MockLlm::new([
        scripted(&request(1), 7),
        scripted(&request(2), 10),
        scripted(&request(1), 4),
    ]);
    while g.dispatch_next(|call| mock.complete(call)).unwrap() {}
    assert_eq!(
        g.ledger(&a).unwrap(),
        coglet_llm::Ledger {
            budget: 20,
            spent: 17,
            reserved: 0
        }
    );
    assert_eq!(g.ledger(&b).unwrap().spent, 4);
    assert_eq!(
        g.enqueue(admission(&a), &request(3)).unwrap_err(),
        "resource_exhausted"
    );
    assert_eq!(mock.calls.len(), 3);
    assert!(g.set_budget(&a, 16).is_err());
}

#[test]
fn uncertainty_survives_restart_no_resend_or_double_debit() {
    let store = Store::new();
    let c = coglet('a');
    let req = request(1);
    let id = req["id"].as_str().unwrap();
    let mut g = store.open();
    g.set_budget(&c, 10).unwrap();
    g.enqueue(admission(&c), &req).unwrap();
    assert!(
        g.dispatch_next(|_| Err("lost provider receipt".into()))
            .is_err()
    );
    drop(g);
    let mut g = store.open();
    assert_eq!(g.ledger(&c).unwrap().reserved, 10);
    assert!(!g.dispatch_next(|_| panic!("blind resend")).unwrap());
    assert!(g.replay(&c, id).is_err());
    assert!(g.enqueue(admission(&c), &request(2)).is_err());
    assert!(g.reconcile(&c, id, success(&req, 11)).is_err());
    assert_eq!(g.ledger(&c).unwrap().reserved, 10);
    g.reconcile(&c, id, success(&req, 6)).unwrap();
    assert!(g.reconcile(&c, id, success(&req, 6)).is_err());
    assert_eq!(g.ledger(&c).unwrap().spent, 6);
    assert_eq!(g.ledger(&c).unwrap().reserved, 0);
}

#[test]
fn definitive_timeout_and_cancellation_are_journaled_failures() {
    for code in ["deadline_exceeded", "cancelled"] {
        let store = Store::new();
        let c = coglet('a');
        let mut g = store.open();
        let req = request(1);
        g.set_budget(&c, 10).unwrap();
        g.enqueue(admission(&c), &req).unwrap();
        g.dispatch_next(|_| {
            Ok(failure(
                req["id"].as_str().unwrap(),
                code,
                "provider confirmed failure",
                2,
            ))
        })
        .unwrap();
        let reply: Value =
            coglet_types_for_test(&g.replay(&c, req["id"].as_str().unwrap()).unwrap());
        assert_eq!(reply["error"]["code"], code);
        assert_eq!(g.ledger(&c).unwrap().spent, 2);
    }
}
fn coglet_types_for_test(raw: &[u8]) -> Value {
    coglet_types::decode_typed("llm-reply", raw).unwrap()
}

#[test]
fn invalid_requests_grants_routes_duplicates_and_receipts_fail_closed() {
    let store = Store::new();
    let c = coglet('a');
    let mut g = store.open();
    g.set_budget(&c, i64::MAX).unwrap();
    let req = request(1);
    assert!(
        g.enqueue(
            Admission {
                coglet: &c,
                llm_complete_granted: false
            },
            &req
        )
        .is_err()
    );
    for bad in [
        json!({}),
        {
            let mut r = req.clone();
            r["input"]["float"] = json!(0.5);
            r
        },
        {
            let mut r = req.clone();
            r["model"] = json!("url-from-guest");
            r
        },
        {
            let mut r = req.clone();
            r["capability"] = json!("llm.other");
            r
        },
    ] {
        assert!(g.enqueue(admission(&c), &bad).is_err());
    }
    assert_eq!(g.ledger(&c).unwrap().reserved, 0);
    g.enqueue(admission(&c), &req).unwrap();
    assert_eq!(g.enqueue(admission(&c), &req).unwrap_err(), "conflict");
    assert!(g.dispatch_next(|_| Ok(success(&request(2), 1))).is_err());
    assert_eq!(g.ledger(&c).unwrap().reserved, 10);
    g.reconcile(&c, req["id"].as_str().unwrap(), success(&req, 10))
        .unwrap();
    assert!(g.enqueue(admission(&c), &req).is_err());
}

#[test]
fn concurrent_connections_share_reservations_and_durable_claims() {
    let store = Store::new();
    let c = coglet('a');
    let mut a = store.open();
    let mut b = store.open();
    a.set_budget(&c, 10).unwrap();
    a.enqueue(admission(&c), &request(1)).unwrap();
    assert!(b.enqueue(admission(&c), &request(2)).is_err());
    assert!(
        a.dispatch_next(|call| {
            assert!(!b.dispatch_next(|_| panic!("duplicate dispatch")).unwrap());
            Ok(success(&call.request, 10))
        })
        .unwrap()
    );
    assert_eq!(b.ledger(&c).unwrap().spent, 10);
}
