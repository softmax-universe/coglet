#![cfg(unix)]
//! Socket-driver component proof, not deployment or real-iroh qualification.
use coglet_hub::{
    Store,
    operator::{
        PeerIdentity, Request,
        server::{Handler, Reply, serve},
        socket::{OperatorSocket, StateLock},
    },
};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{Notify, watch},
};

struct DurableStatus {
    store: Arc<Mutex<Store>>,
    release: Arc<Notify>,
    finished: Arc<Notify>,
}
impl Handler for DurableStatus {
    fn admit(
        &self,
        peer: PeerIdentity,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Reply>> + Send + '_>> {
        Box::pin(async move {
            let store = self.store.clone();
            let release = self.release.clone();
            let finished = self.finished.clone();
            let initial = store
                .lock()
                .unwrap()
                .operator_admit(peer, &request)
                .unwrap();
            let terminal = initial["receipt"]["state"] != "queued";
            let (sender, updates) = watch::channel(initial);
            if terminal {
                return Ok(Reply::Admitted(updates));
            }
            // Fixture executor deliberately owns its task independently of the
            // socket waiter; production's durable saga must provide this seam.
            tokio::spawn(async move {
                release.notified().await;
                let response = store.lock().unwrap().operator_finish(peer.uid, &request, None, |_| Ok(json!({"state":"running","live_commit":null,"step":1,"pending":0,"error":null}))).unwrap();
                sender.send_replace(response);
                finished.notify_one();
            });
            Ok(Reply::Admitted(updates))
        })
    }
}
fn request(timeout: u64) -> Value {
    json!({"version":"local-1","operation_id":"ab".repeat(16),"op":"status","timeout_ms":timeout,
    "params":{"device":"11".repeat(32),"target":format!("coglet:{}","22".repeat(32))}})
}
async fn send(path: &std::path::Path, request: &Value) -> Value {
    let mut client = UnixStream::connect(path).await.unwrap();
    let bytes = serde_json::to_vec(request).unwrap();
    client.write_u32(bytes.len() as u32).await.unwrap();
    client.write_all(&bytes).await.unwrap();
    let n = client.read_u32().await.unwrap();
    let mut bytes = vec![0; n as usize];
    client.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
#[tokio::test]
async fn operator_connection_capacity_precedes_admission() {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Blocked {
        entered: Notify,
        calls: AtomicUsize,
    }
    impl Handler for Blocked {
        fn admit(
            &self,
            _: PeerIdentity,
            _: Request,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Reply>> + Send + '_>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                std::future::pending().await
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let lock = StateLock::acquire(dir.path()).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let handler = Arc::new(Blocked {
        entered: Notify::new(),
        calls: AtomicUsize::new(0),
    });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve(socket, handler.clone(), 1, async {
        let _ = stopped.await;
    }));
    let path = dir.path().join("operator.sock");
    let mut first = UnixStream::connect(&path).await.unwrap();
    let bytes = serde_json::to_vec(&request(300000)).unwrap();
    first.write_u32(bytes.len() as u32).await.unwrap();
    first.write_all(&bytes).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), handler.entered.notified())
        .await
        .unwrap();
    let mut second = UnixStream::connect(&path).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), second.read_u8())
        .await
        .unwrap();
    assert!(
        result.is_err(),
        "saturated client unexpectedly received data"
    );
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(first.read_u8().await.is_err());
}

#[tokio::test]
async fn operator_timeout_only_stops_waiting() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let lock = StateLock::acquire(dir.path()).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(dir.path().join("kernel.db")).unwrap(),
    ));
    let release = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let handler = Arc::new(DurableStatus {
        store: store.clone(),
        release: release.clone(),
        finished: finished.clone(),
    });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve(socket, handler, 2, async {
        let _ = stopped.await;
    }));
    let queued = tokio::time::timeout(
        Duration::from_secs(2),
        send(&dir.path().join("operator.sock"), &request(1)),
    )
    .await
    .unwrap();
    assert_eq!(queued["receipt"]["state"], "queued");
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), finished.notified())
        .await
        .unwrap();
    let req = Request::parse(&serde_json::to_vec(&request(100)).unwrap()).unwrap();
    let uid = std::fs::metadata(dir.path()).unwrap();
    use std::os::unix::fs::MetadataExt;
    let done = store
        .lock()
        .unwrap()
        .operator_admit(
            PeerIdentity {
                uid: uid.uid(),
                gid: None,
                pid: None,
            },
            &req,
        )
        .unwrap();
    assert_eq!(done["receipt"]["state"], "applied");
    let retry = tokio::time::timeout(
        Duration::from_secs(2),
        send(&dir.path().join("operator.sock"), &request(100)),
    )
    .await
    .unwrap();
    assert_eq!(retry, done);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(!dir.path().join("operator.sock").exists());
}
