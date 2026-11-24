//! Device enrollment only. The CLI never impersonates the hub role.
use anyhow::{Context, Result, ensure};
use coglet_proto::transport::{ALPN, Role, Session};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::Path, time::Duration};

pub fn identity(path: &Path) -> Result<Value> {
    let key = SecretKey::generate();
    crate::private_file(path, &key.to_bytes())?;
    Ok(json!({"device":key.public().to_string(),"identity":path}))
}
pub fn load_key(path: &Path) -> Result<SecretKey> {
    let raw = crate::read_private(path, 32)?;
    ensure!(
        raw.len() == 32,
        "identity must contain exactly 32 raw bytes"
    );
    Ok(SecretKey::from_bytes(raw.as_slice().try_into()?))
}
pub async fn enroll_session(session: &Session, invite: Option<&str>) -> Result<Value> {
    if let Some(invite) = invite {
        crate::checked("hub-secret", &json!(invite))
            .map_err(|_| anyhow::anyhow!("invalid invitation format (redacted)"))?;
    }
    let id = hex::encode(&SecretKey::generate().to_bytes()[..16]);
    let reply = session
        .request(
            json!({"method":"hub.enroll","correlation_id":id,"target":null,"timeout_ms":5000,
        "params":{"offer":{"versions":["0.1"]},"invite":invite}}),
        )
        .await
        .map_err(|_| anyhow::anyhow!("enrollment transport failed (details redacted)"))?;
    // Do not print untrusted hub errors: they might echo a secret.
    ensure!(
        reply["type"] == "reply",
        "enrollment refused (use null invite to recover a lost committed success)"
    );
    ensure!(
        reply["result"]["hub"] == session.peer().to_string(),
        "enrollment hub binding mismatch"
    );
    Ok(reply["result"].clone())
}
pub async fn enroll(
    identity: &Path,
    hub: EndpointId,
    address: SocketAddr,
    invite: Option<&str>,
) -> Result<Value> {
    let key = load_key(identity)?;
    let device = key.public();
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .bind()
        .await?;
    let result = async {
        let connection = tokio::time::timeout(
            Duration::from_secs(10),
            endpoint.connect(EndpointAddr::new(hub).with_ip_addr(address), ALPN),
        )
        .await
        .context("hub connect timed out")??;
        let session = Session::new(connection, endpoint.id(), Role::Device)?;
        let result = enroll_session(&session, invite).await?;
        ensure!(
            result["device"] == device.to_string(),
            "enrollment device binding mismatch"
        );
        Ok(result)
    }
    .await;
    endpoint.close().await;
    result
}
