//! Test-only pipe/iroh adapter. Never installs fixture methods in the ALPN.
//! Every harness request crosses the actual encrypted Session into the real Kernel.
use coglet_hub::{EventGrant, Kernel, Registration, Store};
use coglet_proto::{
    authority::Grant,
    transport::{ALPN, Role, Session},
    wire,
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let case = std::env::var("COGLET_CONFORMANCE_CASE")?;
    let expected = match case.as_str() {
        "event-batch-dedup"
        | "event-offset-conflict"
        | "event-source-repeat"
        | "event-empty-probe" => 1,
        "event-source-gap" | "event-retry-a-after-b" => 2,
        "event-gap"
        | "event-runtime-wake"
        | "event-coglet-forged-event"
        | "event-foreign-coglet" => 0,
        _ => return Err("unsupported isolated fixture".into()),
    };
    let scripted: Value =
        serde_json::from_slice(&std::fs::read(format!("conformance/hub/{case}.json"))?)?;
    let requests = scripted["steps"]
        .as_array()
        .ok_or("invalid case")?
        .iter()
        .filter(|s| s.get("send").is_some())
        .count();
    let root = tempfile::tempdir()?;
    let hub = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[2; 32]))
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;
    let device = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[3; 32]))
        .bind()
        .await?;
    let kernel = Kernel::open(
        root.path(),
        hub.id(),
        Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        }),
    )?;
    let signatures: Value =
        serde_json::from_slice(&std::fs::read("spec/fixtures/hub/signatures.json")?)?;
    let mut delegation = signatures[0].clone();
    delegation
        .as_object_mut()
        .ok_or("bad proof")?
        .remove("preimage_hex");
    let target = delegation["statement"]["coglet"]
        .as_str()
        .ok_or("bad target")?
        .to_owned();
    kernel.open_window(Duration::from_secs(60)).await?;
    let invite = kernel
        .invite(
            device.id(),
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.events".into(),
                target: Some(target.clone()),
                expires: 4102444800,
                generation: 0,
            }],
        )
        .await?;
    kernel
        .register(Registration {
            coglet: target.clone(),
            device: device.id().to_string(),
            grant_generation: 0,
            delegation,
            events: vec![EventGrant {
                source: format!("surface:{}", device.id()),
                kind: "test.event".into(),
            }],
            actions: vec![],
        })
        .await?;
    let h = hub.clone();
    let accepting = tokio::spawn(async move {
        Session::new(
            h.accept().await.ok_or("no incoming")?.await?,
            h.id(),
            Role::Hub,
        )
        .map_err(|_| "bad session".into())
            as Result<Session, Box<dyn std::error::Error + Send + Sync>>
    });
    let session = Session::new(
        device.connect(hub.addr(), ALPN).await?,
        device.id(),
        Role::Device,
    )?;
    let remote = accepting.await?.map_err(|_| "accept failed")?;
    let server = tokio::spawn(remote.serve(Arc::new(kernel.clone())));
    let enrolled=session.request(json!({"method":"hub.enroll","target":null,"timeout_ms":2000,"correlation_id":"ff".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}})).await?;
    if enrolled["type"] != "reply" {
        return Err("fixture enrollment failed".into());
    }
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    for index in 0..requests {
        let request = wire::read_frame(&mut input).await?;
        let reply = session.request(request).await?;
        if index + 1 == requests {
            // Stateful, durable oracle: no duplicate admission/rerouting and no
            // effects from rejected batches. It is independent of reply matches.
            let store = Store::open(root.path().join("kernel.db"))?;
            if store.pending_events(&target)?.len() != expected {
                return Err("durable event oracle failed".into());
            }
            let count = store
                .audit()?
                .iter()
                .filter(|r| r["method"] == "hub.events")
                .count();
            if count != expected {
                return Err("durable admission audit oracle failed".into());
            }
        }
        wire::write_frame(&mut output, &reply).await?;
        output.flush().await?;
    }
    device.close().await;
    hub.close().await;
    server.abort();
    Ok(())
}
