//! Opt-in live contract probe: ONLY an explicitly named disposable board.
//! Read credentials from the embedding environment; never print them.
use coglet_factory_host::{HermesAdapter, HttpKanban, TaskSpec};
fn main() {
    let env = |key: &str| std::env::var(key).expect(key);
    let board = env("FACTORY_BOARD");
    assert!(board.starts_with("coglet-factory-") && board != "default");
    let mut adapter = HttpKanban::authenticated(
        env("FACTORY_BOARD_URL"),
        &board,
        &env("FACTORY_BOARD_BEARER"),
        &env("FACTORY_GATEWAY"),
        "factory-contract-fixture",
    )
    .unwrap();
    if env("FACTORY_PROBE_MODE") == "submit" {
        let remote = adapter
            .create(&TaskSpec {
                idempotency_key: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                kind: "factory.contract.fixture".into(),
                payload_cbor_hex: "a0".into(),
                correlation_id: "11111111111111111111111111111111".into(),
                task_id: "11111111111111111111111111111111".into(),
            })
            .unwrap();
        let lease = adapter
            .claim(&remote.remote_id, "factory-host", 60_000, 1)
            .unwrap();
        adapter
            .heartbeat(&remote.remote_id, 61_000, 1, &lease.token)
            .unwrap();
        adapter
            .progress(
                &remote.remote_id,
                "Synthetic Rust adapter live probe; no agent work.",
                1,
                &lease.token,
            )
            .unwrap();
        assert!(
            adapter
                .poll(&remote.remote_id, 1, &lease.token)
                .unwrap()
                .is_none()
        );
        println!(
            "{}",
            serde_json::json!({"remote_id":remote.remote_id,"run_uid":lease.token,"submitted":true})
        );
    } else {
        let remote = env("FACTORY_REMOTE_ID");
        let run = env("FACTORY_RUN_UID");
        let report = adapter.poll(&remote, 1, &run).unwrap().unwrap();
        println!(
            "{}",
            serde_json::json!({"remote_id":remote,"run_uid":run,"outcome":format!("{:?}",report.outcome)})
        );
    }
}
