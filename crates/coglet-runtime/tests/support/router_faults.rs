use super::*;
use coglet_hub::{EventGrant, Kernel, Registration};
use coglet_proto::{
    authority::Grant,
    signing,
    transport::{ALPN, Role, Session},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};

async fn sessions(
    hub: &Endpoint,
    device: &Endpoint,
    kernel: &Kernel,
) -> (
    Session,
    iroh::endpoint::Connection,
    tokio::task::JoinHandle<()>,
) {
    let h = hub.clone();
    let accept = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
    let connection = device.connect(hub.addr(), ALPN).await.unwrap();
    let remote = accept.await.unwrap();
    let client = Session::new(connection.clone(), device.id(), Role::Device).unwrap();
    let server = Session::new(remote, hub.id(), Role::Hub).unwrap();
    let serving = tokio::spawn({
        let server = server.clone();
        let kernel = kernel.clone();
        async move {
            let _ = server.serve(Arc::new(kernel)).await;
        }
    });
    (client, connection, serving)
}
#[tokio::test]
async fn router_iroh_lost_reply_reorder_disconnect_restart_retains_old_batch_receipts() {
    // Overall deadline prevents an injection regression from hanging CI.
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let dir = tempfile::tempdir().unwrap();
    let hub = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let key = SecretKey::from_bytes(&[7; 32]);
    let target = format!("coglet:{}", key.public());
    let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = kernel
        .invite(
            device.id(),
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.events".into(),
                target: Some(target.clone()),
                expires: 1000,
                generation: 3,
            }],
        )
        .await
        .unwrap();
    let (client, server, serving) = sessions(&hub, &device, &kernel).await;
    let enrolled = client.request(json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"1".repeat(32),"params":{"offer":{"versions":["0.1"]},"invite":invite}})).await.unwrap();
    assert_eq!(enrolled["type"], "reply");
    let statement = json!({"purpose":"delegation","coglet":target,"hub":hub.id().to_string(),"device":device.id().to_string(),"generation":9,"expires":1000,"methods":["hub.events","hub.actions"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    kernel.register(Registration { coglet: target.clone(), device: device.id().to_string(), grant_generation: 3,
        delegation: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
        events: vec![EventGrant { source: S.into(), kind: "test.tick".into() }], actions: vec![] }).await.unwrap();
    let batch = |offset: u64| json!({"method":"hub.events","target":target,"timeout_ms":1000,"correlation_id":format!("{:032x}",offset+10),"params":{"offset":offset,"events":[{"source":S,"kind":"test.tick","seq":offset,"logical_time":offset+1,"correlation_id":format!("{:032x}",offset+20),"payload":{}}]}});
    let first = batch(0);
    let later = batch(1);
    let mut link = Link::new();
    link.send(0, first.clone(), Fault::Delay(20));
    link.send(0, later.clone(), Fault::Pass);
    let early = link.drain(0).pop().unwrap();
    let refused = client.request(early).await.unwrap();
    assert_eq!(refused["error"]["code"], "conflict");
    assert_eq!(refused["error"]["details"]["expected_offset"], 0);
    assert!(kernel.events(&target).await.unwrap().is_empty());
    assert!(link.drain(19).is_empty());
    let admitted = client.request(link.drain(20).pop().unwrap()).await.unwrap();
    assert_eq!(admitted["result"]["next_offset"], 1);
    // Drop the reply *after* the real router has persisted admission. This is
    // application-boundary reply loss, not a claim of QUIC packet-loss testing.
    let mut replies = Link::new();
    replies.send(20, admitted.clone(), Fault::Drop);
    assert!(replies.drain(100).is_empty());
    let delivered = kernel.event_deliveries(&target).await.unwrap();
    assert_eq!(delivered.len(), 1);
    kernel.ack_events(&target, delivered[0].0).await.unwrap();
    assert_eq!(
        client.request(later.clone()).await.unwrap()["result"]["next_offset"],
        2
    );
    let delivered = kernel.event_deliveries(&target).await.unwrap();
    assert_eq!(delivered.len(), 1);
    kernel.ack_events(&target, delivered[0].0).await.unwrap();
    server.close(0u32.into(), b"injected disconnect");
    serving.abort();
    let _ = serving.await;
    drop(server);
    drop(client);
    drop(kernel);
    let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
    let (client, server, serving) = sessions(&hub, &device, &kernel).await;
    let enrolled = client.request(json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"e".repeat(32),"params":{"offer":{"versions":["0.1"]},"invite":null}})).await.unwrap();
    assert_eq!(enrolled["type"], "reply", "{enrolled}");
    let mut retry = first.clone();
    retry["correlation_id"] = json!("f".repeat(32));
    let recovered = client.request(retry.clone()).await.unwrap();
    assert_eq!(recovered["result"], admitted["result"], "{recovered}");
    assert!(
        kernel.events(&target).await.unwrap().is_empty(),
        "old retry redelivered after later admission + restart"
    );
    retry["params"]["events"][0]["payload"] = json!({"changed":true});
    assert_eq!(
        client.request(retry).await.unwrap()["error"]["code"],
        "conflict"
    );
    assert!(kernel.events(&target).await.unwrap().is_empty());
    assert_eq!(
        client.request(later).await.unwrap()["result"]["next_offset"],
        2
    );
    server.close(0u32.into(), b"injected disconnect");
    serving.abort();
    let _ = serving.await;
    device.close().await;
    hub.close().await;
}
