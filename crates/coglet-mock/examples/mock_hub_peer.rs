//! Test-local stdio bridge. Requests are unchanged and all cross encrypted iroh.
use coglet_mock::hub::{MockHub, Result};
use coglet_proto::{
    authority::Grant,
    transport::{ALPN, Role, Session},
    wire,
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<()> {
    let case = std::env::var("COGLET_CONFORMANCE_CASE")?;
    if !matches!(
        case.as_str(),
        "enroll-reconnect" | "health-minimal" | "enroll-unsupported-version"
    ) {
        return Err("unsupported isolated fixture".into());
    }
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(format!("conformance/hub/{case}.json"))?)?;
    let requests = fixture["steps"]
        .as_array()
        .ok_or("bad case")?
        .iter()
        .filter(|s| s.get("send").is_some())
        .count();
    let hub = MockHub::start().await?;
    let d = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[3; 32]))
        .bind()
        .await?;
    if case != "enroll-unsupported-version" {
        let token = hub
            .invite(
                d.id(),
                Duration::from_secs(60),
                vec![Grant {
                    method: "hub.health".into(),
                    target: None,
                    expires: u64::MAX,
                    generation: 0,
                }],
            )
            .await?;
        let c = d.connect(hub.addr(), ALPN).await?;
        let s = Session::new(c.clone(), d.id(), Role::Device)?;
        let r = s.request(json!({"method":"hub.enroll","target":null,"timeout_ms":2000,"correlation_id":"ff".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":token}})).await?;
        if r["type"] != "reply" {
            return Err("fixture enrollment failed".into());
        }
        c.close(0u32.into(), b"fixture reconnect");
        c.closed().await;
    }
    let s = Session::new(d.connect(hub.addr(), ALPN).await?, d.id(), Role::Device)?;
    if case == "health-minimal" {
        let r = s.request(json!({"method":"hub.enroll","target":null,"timeout_ms":2000,"correlation_id":"fe".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":null}})).await?;
        if r["type"] != "reply" {
            return Err("fixture recovery failed".into());
        }
    }
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    for _ in 0..requests {
        let request = wire::read_frame(&mut input).await?;
        let reply = s.request(request).await?;
        wire::write_frame(&mut output, &reply).await?;
        output.flush().await?;
    }
    d.close().await;
    hub.shutdown().await;
    Ok(())
}
