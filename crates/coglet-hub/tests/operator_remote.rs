//! The local operator vocabulary is not an iroh admin API.
use coglet_hub::{
    Kernel,
    operator::{
        server,
        service::{Provisioning, Service},
        socket::{OperatorSocket, StateLock},
    },
};
use coglet_proto::transport::{ALPN, Role, Session};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_no_remote_listener() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let hub = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let device = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let kernel = Kernel::open(root.path(), hub.id(), Arc::new(|| Some(10))).unwrap();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = kernel
        .invite(device.id(), Duration::from_secs(60), vec![])
        .await
        .unwrap();
    let uid = std::fs::metadata(root.path()).unwrap().uid();
    let provisioning:Provisioning=serde_json::from_value(json!({"uid":uid,"actor":format!("hub:{}",hub.id()),"operations":["enrollment.window"],"max_window_ms":1000,"grants":[]})).unwrap();
    let service = Arc::new(Service::new(kernel.clone(), uid, provisioning).unwrap());
    let (outgoing, incoming) = tokio::join!(device.connect(hub.addr(), ALPN), async {
        hub.accept().await.unwrap().await.unwrap()
    });
    let outgoing = outgoing.unwrap();
    let remote = Session::new(outgoing.clone(), device.id(), Role::Device).unwrap();
    let local = Session::new(incoming, hub.id(), Role::Hub).unwrap();
    let token = kernel.transport_connected(device.id()).await.unwrap();
    let handler = service.connection(local.clone(), token);
    let serving = tokio::spawn(local.serve(handler));
    let enrolled=remote.request(json!({"method":"hub.enroll","target":null,"correlation_id":"11".repeat(16),"timeout_ms":5000,"params":{"invite":invite,"offer":{"versions":["0.1"]}}})).await.unwrap();
    assert_eq!(enrolled["type"], "reply");
    let before = kernel.audit().await.unwrap();
    for method in [
        "deploy",
        "push",
        "stop",
        "status",
        "grant.provision",
        "grant.revoke",
        "enrollment.window",
        "enrollment.invite",
    ] {
        // Bypass Session's caller-side schema check: observe the actual receiver
        // reject the closed wire union, even after authenticated enrollment.
        let (mut send, mut recv) = outgoing.open_bi().await.unwrap();
        coglet_proto::wire::write_frame(&mut send,&json!({"method":method,"target":null,"correlation_id":"22".repeat(16),"timeout_ms":5000,"params":{}})).await.unwrap();
        let _ = send.finish();
        let result = tokio::time::timeout(Duration::from_secs(5), recv.read(&mut [0; 1]))
            .await
            .unwrap();
        assert!(
            matches!(result,Err(iroh::endpoint::ReadError::Reset(code)) if code==1u32.into()),
            "remote {method} was not reset before dispatch: {result:?}"
        );
    }
    assert_eq!(
        kernel.audit().await.unwrap(),
        before,
        "remote admin reached kernel/operator admission"
    );
    let lock = StateLock::acquire(root.path()).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let (stop, rx) = tokio::sync::oneshot::channel();
    let socket_task = tokio::spawn(server::serve(socket, service, 8, async {
        let _ = rx.await;
    }));
    let mut client = tokio::net::UnixStream::connect(root.path().join("operator.sock"))
        .await
        .unwrap();
    let bytes=serde_json::to_vec(&json!({"version":"local-1","operation_id":"22".repeat(16),"op":"enrollment.window","timeout_ms":1000,"params":{"duration_ms":0}})).unwrap();
    client.write_u32(bytes.len() as u32).await.unwrap();
    client.write_all(&bytes).await.unwrap();
    let n = client.read_u32().await.unwrap();
    let mut bytes = vec![0; n as usize];
    client.read_exact(&mut bytes).await.unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        reply["receipt"]["state"], "applied",
        "local positive control: {reply}"
    );
    stop.send(()).unwrap();
    socket_task.await.unwrap().unwrap();
    serving.abort();
    let _ = serving.await;
    device.close().await;
    hub.close().await;
}
