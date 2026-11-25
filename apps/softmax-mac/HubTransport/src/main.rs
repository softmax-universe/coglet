//! Secrets travel only through inherited anonymous pipes, never argv/env/files.
use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey};
use serde::Deserialize;
use serde_json::{Value, json};
use softmax_hub_link::{Result, connect, endpoint};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pairing {
    hub: String,
    addresses: Vec<std::net::SocketAddr>,
    relay: Option<String>,
    invite: Option<String>,
}
async fn read(input: &mut BufReader<tokio::io::Stdin>) -> Result<Option<Value>> {
    // Bounded before parsing, including accidental QR/image/clipboard payloads.
    use tokio::io::AsyncReadExt;
    let mut line = Vec::new();
    let count = (&mut *input)
        .take(16385)
        .read_until(b'\n', &mut line)
        .await?;
    if count == 0 {
        return Ok(None);
    }
    if count > 16384 || line.last() != Some(&b'\n') {
        return Err("oversized command".into());
    }
    Ok(Some(serde_json::from_slice(&line)?))
}
async fn emit(value: Value) -> Result<()> {
    let mut out = tokio::io::stdout();
    out.write_all(format!("{value}\n").as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
async fn run() -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin());
    let first = read(&mut input).await?.ok_or("missing identity")?;
    let bytes: [u8; 32] = hex::decode(first["key"].as_str().ok_or("missing key")?)?
        .try_into()
        .map_err(|_| "invalid key")?;
    let device = endpoint(SecretKey::from_bytes(&bytes)).await?;
    emit(json!({"state":"ready", "device":device.id().to_string()})).await?;
    while let Some(command) = read(&mut input).await? {
        let pairing: Pairing = serde_json::from_value(command)?;
        let mut addr = EndpointAddr::new(pairing.hub.parse::<EndpointId>()?);
        for a in pairing.addresses {
            addr = addr.with_ip_addr(a);
        }
        if let Some(relay) = pairing.relay {
            addr = addr.with_relay_url(relay.parse::<RelayUrl>()?);
        }
        // Discover/pinned-only addressing is legal; never trust a different hub ID.
        let result = connect(&device, addr.clone(), None).await;
        let result = match (result, pairing.invite) {
            (Err(_), Some(invite)) => connect(&device, addr, Some(invite)).await,
            (other, _) => other,
        };
        let connection = match result {
            Ok(c) => c,
            Err(_) => {
                emit(json!({"state":"offline"})).await?;
                continue;
            }
        };
        emit(json!({"state":"connected"})).await?;
        tokio::select! {
            _ = connection.closed() => {},
            command = read(&mut input) => {
                connection.close(0u32.into(), b"local disconnect");
                if command?.is_none() { break; }
            }
        }
        emit(json!({"state":"offline"})).await?;
    }
    device.close().await;
    Ok(())
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        // Do not print serde errors: input can contain a private invite/key.
        let _ = emit(json!({"state":"failed"})).await;
        std::process::exit(1);
    }
}
