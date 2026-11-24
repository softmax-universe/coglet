use coglet_mock::hub::{MockHub, Result};
use coglet_proto::{
    authority::Grant,
    transport::{ALPN, Role, Session},
};
use iroh::{
    Endpoint, SecretKey,
    endpoint::{Connection, presets},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

// Scheduler budgets are deliberately shared process-wide by (local, peer).
// Give concurrent independent fixtures distinct deterministic hub identities;
// reconnects within each fixture still use exactly the same identity.
async fn hub(seed: u8) -> Result<MockHub> {
    MockHub::with_clock(
        SecretKey::from_bytes(&[seed; 32]),
        Arc::new(|| Some(1_800_000_000)),
    )
    .await
}

async fn device(seed: u8) -> Result<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[seed; 32]))
        .bind()
        .await?)
}
async fn connect(hub: &MockHub, device: &Endpoint) -> Result<(Connection, Session)> {
    let c = device.connect(hub.addr(), ALPN).await?;
    Ok((c.clone(), Session::new(c, device.id(), Role::Device)?))
}
fn enroll(invite: Option<&str>, id: u8) -> Value {
    json!({"method":"hub.enroll","target":null,"timeout_ms":2000,"correlation_id":format!("{id:032x}"),"params":{"offer":{"versions":["0.1"]},"invite":invite}})
}
fn health(device: &Endpoint) -> Value {
    json!({"method":"hub.health","target":null,"timeout_ms":2000,"correlation_id":"aa".repeat(16),"params":{"device":device.id().to_string(),"uptime_ms":1,"queue_depth":0,"available_bytes":1048576,"coglets":[]}})
}
async fn invite(hub: &MockHub, device: &Endpoint) -> Result<String> {
    hub.invite(
        device.id(),
        Duration::from_secs(60),
        vec![Grant {
            method: "hub.health".into(),
            target: None,
            expires: u64::MAX,
            generation: 0,
        }],
    )
    .await
}
async fn state(hub: &MockHub, device: &Endpoint, expected: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let status = hub.kernel().status().await?;
            if status["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["device"] == device.id().to_string() && d["state"] == expected)
            {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn enrollment_is_identity_bound_and_one_use() -> Result<()> {
    let hub = hub(11).await?;
    let d = device(3).await?;
    let other = device(4).await?;
    let token = invite(&hub, &d).await?;
    let (_, wrong) = connect(&hub, &other).await?;
    assert_eq!(
        wrong.request(enroll(Some(&token), 1)).await?["error"]["code"],
        "unauthenticated"
    );
    let (_, s) = connect(&hub, &d).await?;
    let result = s.request(enroll(Some(&token), 2)).await?;
    assert_eq!(result["result"]["selection"]["version"], "0.1");
    assert_eq!(
        s.request(enroll(Some(&token), 3)).await?["error"]["code"],
        "unauthenticated"
    );
    assert_eq!(s.request(enroll(None, 4)).await?["type"], "reply");
    other.close().await;
    d.close().await;
    hub.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn connect_requires_authenticated_enrollment() -> Result<()> {
    let hub = hub(12).await?;
    let d = device(3).await?;
    let token = invite(&hub, &d).await?;
    let (_, s) = connect(&hub, &d).await?;
    assert_eq!(
        s.request(health(&d)).await?["error"]["code"],
        "unauthenticated"
    );
    assert_eq!(s.request(enroll(Some(&token), 1)).await?["type"], "reply");
    assert_eq!(s.request(health(&d)).await?["type"], "reply");
    state(&hub, &d, "connected").await?;
    d.close().await;
    hub.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn disconnect_updates_kernel_connection_status() -> Result<()> {
    let hub = hub(13).await?;
    let d = device(3).await?;
    let token = invite(&hub, &d).await?;
    let (c, s) = connect(&hub, &d).await?;
    assert_eq!(s.request(enroll(Some(&token), 1)).await?["type"], "reply");
    state(&hub, &d, "connected").await?;
    c.close(0u32.into(), b"test disconnect");
    state(&hub, &d, "disconnected").await?;
    d.close().await;
    hub.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn reconnect_recovers_same_identity_without_reusing_invite() -> Result<()> {
    let hub = hub(14).await?;
    let d = device(3).await?;
    let token = invite(&hub, &d).await?;
    let (c, s) = connect(&hub, &d).await?;
    assert_eq!(s.request(enroll(Some(&token), 1)).await?["type"], "reply");
    c.close(0u32.into(), b"lost enrollment success");
    state(&hub, &d, "disconnected").await?;
    d.close().await;
    let recovered = device(3).await?;
    assert_eq!(d.id(), recovered.id());
    let (_, s) = connect(&hub, &recovered).await?;
    assert_eq!(
        s.request(health(&recovered)).await?["error"]["code"],
        "unauthenticated"
    );
    assert_eq!(s.request(enroll(None, 2)).await?["type"], "reply");
    assert_eq!(s.request(health(&recovered)).await?["type"], "reply");
    state(&hub, &recovered, "connected").await?;
    recovered.close().await;
    hub.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn revoked_and_unknown_identities_are_refused() -> Result<()> {
    let hub = hub(15).await?;
    let d = device(3).await?;
    let (_, unknown) = connect(&hub, &d).await?;
    assert_eq!(
        unknown.request(enroll(None, 1)).await?["error"]["code"],
        "unauthenticated"
    );
    let token = invite(&hub, &d).await?;
    assert_eq!(
        unknown.request(enroll(Some(&token), 2)).await?["type"],
        "reply"
    );
    hub.kernel().revoke_peer(d.id()).await?;
    assert_ne!(unknown.request(health(&d)).await?["type"], "reply");
    let (_, fresh) = connect(&hub, &d).await?;
    assert_eq!(
        fresh.request(enroll(None, 3)).await?["error"]["code"],
        "unauthenticated"
    );
    assert_eq!(
        fresh.request(enroll(Some(&token), 4)).await?["error"]["code"],
        "unauthenticated"
    );
    d.close().await;
    hub.shutdown().await;
    Ok(())
}
