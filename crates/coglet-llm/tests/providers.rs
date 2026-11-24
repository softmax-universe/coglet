use coglet_llm::http::{Api, HttpConfig, HttpProvider};
use coglet_llm::{Call, Provider};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};
fn fake(status: u16, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = body.to_owned();
    let join = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut raw = Vec::new();
        let mut buf = [0; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            raw.extend_from_slice(&buf[..n]);
            if let Some(pos) = raw.windows(4).position(|x| x == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&raw[..pos]);
                let len: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|s| s.parse().unwrap())
                    })
                    .unwrap();
                if raw.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        String::from_utf8(raw).unwrap()
    });
    (format!("http://{addr}/v1/complete"), join)
}
fn config(api: Api, url: String) -> HttpConfig {
    HttpConfig {
        api,
        endpoint: url,
        model: "trusted-model".into(),
        secret_name: "hub/provider".into(),
        max_output_tokens: 10,
        max_prompt_bytes: 100,
        input_micro_per_token: 1,
        output_micro_per_token: 2,
        timeout: Duration::from_secs(2),
    }
}
fn call() -> Call {
    Call {
        coglet: format!("coglet:{}", "a".repeat(64)),
        request: json!({"id":"1".repeat(32),"input":{"prompt":"hi"}}),
        provider: "configured".into(),
        ceiling: 1144,
    }
}
#[test]
fn provider_uncertainty_keeps_reservation_across_restart_without_resend() {
    use coglet_llm::{Admission, Gateway, Route};
    use std::collections::BTreeMap;
    let dir = std::env::temp_dir().join(format!("coglet-llm-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("gateway.db");
    let routes = || {
        BTreeMap::from([(
            "small".into(),
            Route {
                provider: "configured".into(),
                ceiling: 1144,
            },
        )])
    };
    let mut g = Gateway::open(&path, routes()).unwrap();
    let c = call().coglet;
    g.set_budget(&c, 1144).unwrap();
    let req = json!({"id":"1".repeat(32),"correlation_id":"c".repeat(32),"capability":"llm.complete","model":"small","input":{"prompt":"hi"}});
    g.enqueue(
        Admission {
            coglet: &c,
            llm_complete_granted: true,
        },
        &req,
    )
    .unwrap();
    let (url, join) = fake(503, "secret-body");
    let mut p = HttpProvider::new(config(Api::OpenAi, url), |_: &str| Ok("key".into())).unwrap();
    assert!(g.dispatch_next(|c| p.complete(c)).is_err());
    join.join().unwrap();
    drop(g);
    let mut g = Gateway::open(&path, routes()).unwrap();
    assert_eq!(g.ledger(&c).unwrap().reserved, 1144);
    assert!(!g.dispatch_next(|_| panic!("blind resend")).unwrap());
    drop(g);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unsafe_endpoints_and_pricing_are_rejected_before_secret_lookup() {
    for url in [
        "http://example.com/v1/messages",
        "https://user:pass@example.com/",
        "https://example.com/?key=secret",
        "https://example.com/#secret",
    ] {
        assert!(
            HttpProvider::new(
                config(Api::OpenAi, url.into()),
                |_: &str| -> coglet_llm::Result<String> { panic!("lookup at setup") }
            )
            .is_err()
        );
    }
}
#[test]
#[ignore = "optional paid live smoke; explicit secret and local pool proxy required"]
fn optional_live_smoke_gated_on_secret() {
    let key = std::env::var("COGLET_LLM_SMOKE_KEY").expect("explicit smoke secret required");
    let base = std::env::var("ANTHROPIC_BASE_URL").expect("start hermes-pool-llm serve first");
    assert!(
        base.starts_with("http://127.0.0.1:") || base.starts_with("http://[::1]:"),
        "smoke must use local pool proxy"
    );
    let mut cfg = config(
        Api::Anthropic,
        format!("{}/v1/messages", base.trim_end_matches('/')),
    );
    cfg.model = std::env::var("COGLET_LLM_SMOKE_MODEL").expect("explicit smoke model required");
    cfg.timeout = Duration::from_secs(60);
    let mut p = HttpProvider::new(cfg, move |_: &str| Ok(key.clone())).unwrap();
    let reply = p.complete(&call()).unwrap();
    assert_eq!(reply.reply["status"], "succeeded");
}

#[test]
fn malformed_or_over_budget_receipts_never_settle_as_success() {
    for body in [
        r#"{}"#,
        r#"{"choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":3,"completion_tokens":11}}"#,
        r#"{"choices":[{"message":{"content":"hi"}}],"usage":{"completion_tokens":2}}"#,
    ] {
        let (url, join) = fake(200, body);
        let mut p = HttpProvider::new(config(Api::OpenAi, url), |_: &str| Ok("fixture-key".into()))
            .unwrap();
        assert!(p.complete(&call()).is_err());
        join.join().unwrap();
    }
    let mut p = HttpProvider::new(
        config(Api::OpenAi, "http://127.0.0.1:1/".into()),
        |_: &str| -> coglet_llm::Result<String> { panic!("insufficient reservation dispatched") },
    )
    .unwrap();
    let mut too_small = call();
    too_small.ceiling = 1;
    assert_eq!(
        p.complete(&too_small).unwrap().reply["error"]["code"],
        "resource_exhausted"
    );
}
#[test]
fn openai_local_fake_uses_secret_store_and_accounts_usage() {
    let (url, join) = fake(
        200,
        r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
    );
    let mut p = HttpProvider::new(config(Api::OpenAi, url), |name: &str| {
        assert_eq!(name, "hub/provider");
        Ok("fixture-key".to_owned())
    })
    .unwrap();
    let result = p.complete(&call()).unwrap();
    assert_eq!(result.cost, 7);
    assert_eq!(result.reply["output"]["text"], "hello");
    let request = join.join().unwrap();
    assert!(request.contains("Bearer fixture-key"));
    assert!(request.contains("trusted-model"));
    assert!(request.contains("max_completion_tokens"));
}
#[test]
fn anthropic_local_fake_maps_text_and_usage() {
    let (url, join) = fake(
        200,
        r#"{"content":[{"type":"text","text":"héllo"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}"#,
    );
    let mut p = HttpProvider::new(config(Api::Anthropic, url), |_: &str| {
        Ok("fixture-key".to_owned())
    })
    .unwrap();
    let result = p.complete(&call()).unwrap();
    assert_eq!(result.cost, 7);
    assert_eq!(result.reply["output"]["text"], "héllo");
    let request = join.join().unwrap();
    assert!(request.contains("x-api-key: fixture-key"));
    assert!(request.contains("anthropic-version: 2023-06-01"));
}
#[test]
fn provider_error_mapping_never_leaks_body_or_retries_ambiguous_errors() {
    for (status, code) in [
        (400, "invalid_argument"),
        (401, "permission_denied"),
        (403, "permission_denied"),
        (429, "resource_exhausted"),
    ] {
        let (url, join) = fake(status, r#"{"error":{"message":"SECRET-do-not-record"}}"#);
        let mut p = HttpProvider::new(config(Api::OpenAi, url), |_: &str| Ok("fixture-key".into()))
            .unwrap();
        let reply = p.complete(&call()).unwrap();
        assert_eq!(reply.cost, 0);
        assert_eq!(reply.reply["error"]["code"], code);
        assert!(!reply.reply.to_string().contains("SECRET"));
        join.join().unwrap();
    }
    let (url, join) = fake(503, "SECRET");
    let mut p =
        HttpProvider::new(config(Api::OpenAi, url), |_: &str| Ok("fixture-key".into())).unwrap();
    assert_eq!(
        p.complete(&call()).err().unwrap(),
        "uncertain provider response"
    );
    join.join().unwrap();
}
