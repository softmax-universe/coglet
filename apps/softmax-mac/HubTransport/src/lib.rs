//! App-owned transport; protocol validation and negotiation stay in coglet-proto.
use coglet_proto::transport::{ALPN, Role, Session};
use iroh::{
    Endpoint, EndpointAddr, SecretKey,
    endpoint::{Connection, presets},
};
use serde_json::json;
use std::time::Duration;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub async fn endpoint(key: SecretKey) -> Result<Endpoint> {
    Ok(Endpoint::builder(presets::N0)
        .secret_key(key)
        .bind()
        .await?)
}

pub async fn connect(
    device: &Endpoint,
    hub: EndpointAddr,
    invite: Option<String>,
) -> Result<Connection> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let c = device.connect(hub, ALPN).await?;
        let session = Session::new(c.clone(), device.id(), Role::Device)?;
        let reply = session
            .request(json!({
                "method":"hub.enroll", "target":null, "timeout_ms":10000,
                "correlation_id": SecretKey::generate().public().to_string()[..32],
                "params":{"offer":{"versions":["0.1"]},"invite":invite}
            }))
            .await?;
        if reply["result"]["selection"]["version"] != "0.1" {
            c.close(1u32.into(), b"enrollment refused");
            return Err("enrollment refused".into());
        }
        Ok(c)
    })
    .await?
}
