//! Local test launcher. Private stdout: one JSON bootstrap line, never a log.
//! Usage: cargo run -p coglet-mock --example hub -- DEVICE_NODE_ID
//! Close stdin (EOF) or send SIGINT to stop. No private client key is supplied.
use coglet_mock::hub::{MockHub, Result};
use coglet_proto::authority::Grant;
use serde_json::json;
use std::{io::Write, time::Duration};
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let device = args.next().ok_or("usage: hub DEVICE_NODE_ID")?.parse()?;
    if args.next().is_some() {
        return Err("usage: hub DEVICE_NODE_ID".into());
    }
    let hub = MockHub::start().await?;
    let invite = hub
        .invite(
            device,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.health".into(),
                target: None,
                expires: u64::MAX,
                generation: 0,
            }],
        )
        .await?;
    println!(
        "{}",
        json!({
            "hub": hub.id().to_string(),
            "alpn": "coglet/hub/0",
            "addresses": hub.addr().ip_addrs().map(ToString::to_string).collect::<Vec<_>>(),
            "device": device.to_string(), "invite": invite,
        })
    );
    std::io::stdout().flush()?;
    let mut stdin = tokio::io::stdin();
    let mut bytes = [0; 1];
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = async {
            while stdin.read(&mut bytes).await.unwrap_or(0) != 0 {
                if bytes[0] == b'd' {
                    let closed = hub.disconnect_device(device).await;
                    eprintln!("hub-side disconnect: {closed}");
                }
            }
        } => {},
    }
    hub.shutdown().await;
    Ok(())
}
