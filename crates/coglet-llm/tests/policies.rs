use coglet_llm::{Admission, Completion, Gateway, Route};
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn coglet(c: char) -> String {
    format!("coglet:{}", c.to_string().repeat(64))
}
fn req(n: u64) -> Value {
    json!({"id":format!("{n:032x}"),"correlation_id":"c".repeat(32),"capability":"llm.complete","model":"small","input":{"prompt":"hello"}})
}
fn auth(c: &str) -> Admission<'_> {
    Admission {
        coglet: c,
        llm_complete_granted: true,
    }
}
fn open(path: &str) -> Gateway {
    Gateway::open(
        path,
        BTreeMap::from([(
            "small".into(),
            Route {
                provider: "fake".into(),
                ceiling: 10,
            },
        )]),
    )
    .unwrap()
}
fn ok(call: &coglet_llm::Call) -> Completion {
    Completion {
        reply: json!({"request_id":call.request["id"],"status":"succeeded","output":{"text":"hello"}}),
        cost: 7,
    }
}
#[test]
fn rate_and_cache_survive_reopen_and_share_connection_policy() {
    let dir = std::env::temp_dir().join(format!("coglet-llm-policy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.db");
    let path = path.to_str().unwrap();
    let a = coglet('a');
    let mut g = open(path);
    g.set_budget(&a, 100).unwrap();
    g.set_rate_limit(&a, 1, 1000).unwrap();
    g.set_cache_revision(&a, Some("v1")).unwrap();
    g.enqueue_at(auth(&a), &req(1), 100).unwrap();
    g.dispatch_next(|c| Ok(ok(c))).unwrap();
    drop(g);
    let mut g = open(path);
    let mut other = open(path);
    assert!(other.enqueue_at(auth(&a), &req(2), 100).is_err());
    g.enqueue_at(auth(&a), &req(2), 1100).unwrap();
    assert!(other.enqueue_at(auth(&a), &req(3), 1100).is_err());
    other
        .dispatch_next(|_| panic!("reopened cache miss"))
        .unwrap();
    assert_eq!(g.ledger(&a).unwrap().spent, 7);
    drop(other);
    drop(g);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn cache_hit_still_requires_current_authorization() {
    let mut g = open(":memory:");
    let a = coglet('a');
    g.set_budget(&a, 100).unwrap();
    g.set_cache_revision(&a, Some("v1")).unwrap();
    g.enqueue(auth(&a), &req(1)).unwrap();
    g.dispatch_next(|c| Ok(ok(c))).unwrap();
    g.enqueue(auth(&a), &req(2)).unwrap();
    assert!(
        g.dispatch_next_checked(|_| Err("revoked".into()), |_| panic!("revoked network"))
            .is_err()
    );
    assert!(g.replay(&a, &format!("{:032x}", 2)).is_err());
    assert_eq!(g.ledger(&a).unwrap().reserved, 10);
}

#[test]
fn rate_limit_is_atomic_per_coglet_and_backward_clock_fails_closed() {
    let mut g = open(":memory:");
    let a = coglet('a');
    let b = coglet('b');
    for c in [&a, &b] {
        g.set_budget(c, 100).unwrap();
        g.set_rate_limit(c, 2, 1000).unwrap();
    }
    g.enqueue_at(auth(&a), &req(1), 100).unwrap();
    g.enqueue_at(auth(&a), &req(2), 100).unwrap();
    assert_eq!(
        g.enqueue_at(auth(&a), &req(3), 999).unwrap_err(),
        "resource_exhausted: rate limit"
    );
    g.enqueue_at(auth(&b), &req(1), 999).unwrap();
    assert!(g.enqueue_at(auth(&a), &req(3), 99).is_err());
    g.enqueue_at(auth(&a), &req(3), 1100).unwrap();
    assert_eq!(g.ledger(&a).unwrap().reserved, 30);
}
#[test]
fn response_cache_hit_rebinds_identity_without_spend_and_is_tenant_isolated() {
    let mut g = open(":memory:");
    let a = coglet('a');
    let b = coglet('b');
    for c in [&a, &b] {
        g.set_budget(c, 100).unwrap();
        g.set_cache_revision(c, Some("pricing-and-prompt-v1"))
            .unwrap();
    }
    g.enqueue(auth(&a), &req(1)).unwrap();
    g.dispatch_next(|c| Ok(ok(c))).unwrap();
    g.enqueue(auth(&a), &req(2)).unwrap();
    g.dispatch_next(|_| panic!("cache called network")).unwrap();
    let reply: Value =
        coglet_types::decode_typed("llm-reply", &g.replay(&a, &format!("{:032x}", 2)).unwrap())
            .unwrap();
    assert_eq!(reply["request_id"], req(2)["id"]);
    assert_eq!(g.ledger(&a).unwrap().spent, 7);
    assert_eq!(g.ledger(&a).unwrap().reserved, 0);
    g.enqueue(auth(&b), &req(1)).unwrap();
    let mut calls = 0;
    g.dispatch_next(|c| {
        calls += 1;
        Ok(ok(c))
    })
    .unwrap();
    assert_eq!(calls, 1);
    g.set_cache_revision(&a, Some("v2")).unwrap();
    g.enqueue(auth(&a), &req(3)).unwrap();
    g.dispatch_next(|c| {
        calls += 1;
        Ok(ok(c))
    })
    .unwrap();
    assert_eq!(calls, 2);
}
