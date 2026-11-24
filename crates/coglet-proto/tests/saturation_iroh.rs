use coglet_proto::{
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
    wire::{read_frame, write_frame},
};
use iroh::{
    Endpoint,
    endpoint::{Connection, RecvStream, presets},
};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::{Semaphore, mpsc};

struct Handler {
    entered: mpsc::UnboundedSender<String>,
    release: Semaphore,
}
impl OperationHandler for Handler {
    fn handle<'a>(
        &'a self,
        ctx: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            let method = request["method"].as_str().unwrap();
            if method == "hub.enroll" {
                return json!({"type":"reply","correlation_id":request["correlation_id"],"result":{"selection":{"version":"0.1"},"device":ctx.peer.to_string(),"hub":ctx.local.to_string()}});
            }
            self.entered.send(method.to_owned()).unwrap();
            assert_ne!(method, "debug.pause", "injected handler panic");
            if matches!(
                method,
                "hub.status" | "hub.git.fetch" | "hub.events" | "debug.write"
            ) {
                let _permit = self.release.acquire().await.unwrap();
            }
            json!({"type":"error","correlation_id":request["correlation_id"],"error":{"code":"permission_denied","message":"test handler reached","retryable":false}})
        })
    }
}
fn request(method: &str, hub: &Endpoint, device: &Endpoint) -> Value {
    let params = if matches!(method, "hub.cancel" | "hub.receipt") {
        json!({"operation_id":"cd".repeat(16)})
    } else {
        json!({})
    };
    json!({"method":method,"correlation_id":"ab".repeat(16),"target":format!("coglet:{}",device.id()),"timeout_ms":10000,"params":params,"context":{"actor":format!("hub:{}",hub.id())}})
}
struct Fixture {
    hub: Endpoint,
    device: Endpoint,
    connections: Vec<Connection>,
    reverse: Vec<Connection>,
    tasks: Vec<tokio::task::JoinHandle<Result<(), coglet_proto::transport::TransportError>>>,
    handler: Arc<Handler>,
    entered: mpsc::UnboundedReceiver<String>,
}
impl Fixture {
    async fn new() -> Self {
        let hub = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let (tx, entered) = mpsc::unbounded_channel();
        let handler = Arc::new(Handler {
            entered: tx,
            release: Semaphore::new(0),
        });
        let mut result = Self {
            hub,
            device,
            connections: vec![],
            reverse: vec![],
            tasks: vec![],
            handler,
            entered,
        };
        for _ in 0..2 {
            let h = result.hub.clone();
            let accept = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
            let dc = result
                .device
                .connect(result.hub.addr(), ALPN)
                .await
                .unwrap();
            let hc = accept.await.unwrap();
            let hs = Session::new(hc.clone(), result.hub.id(), Role::Hub).unwrap();
            let ds = Session::new(dc.clone(), result.device.id(), Role::Device).unwrap();
            result
                .tasks
                .push(tokio::spawn(hs.serve(result.handler.clone())));
            let enroll = json!({"method":"hub.enroll","correlation_id":"ef".repeat(16),"target":null,"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":null}});
            assert_eq!(ds.request(enroll).await.unwrap()["type"], "reply");
            result
                .tasks
                .push(tokio::spawn(ds.serve(result.handler.clone())));
            result.connections.push(hc);
            result.reverse.push(dc);
        }
        assert_ne!(
            result.connections[0].stable_id(),
            result.connections[1].stable_id()
        );
        result
    }
    async fn reply(&self, connection: usize, value: &Value) -> Value {
        let mut recv = raw(&self.connections[connection], value).await;
        tokio::time::timeout(Duration::from_secs(1), read_frame(&mut recv))
            .await
            .unwrap()
            .unwrap()
    }
    async fn close(self) {
        self.device.close().await;
        self.hub.close().await;
        for task in self.tasks {
            task.abort();
        }
    }
}
async fn raw(connection: &Connection, value: &Value) -> RecvStream {
    let (mut send, recv) = connection.open_bi().await.unwrap();
    write_frame(&mut send, value).await.unwrap();
    send.finish().unwrap();
    recv
}
#[tokio::test]
async fn handler_panic_and_serve_abort_reset_streams_and_release_shared_capacity() {
    let mut f = Fixture::new().await;
    for _ in 0..17 {
        let mut recv = raw(
            &f.connections[0],
            &request("debug.pause", &f.hub, &f.device),
        )
        .await;
        assert_eq!(f.entered.recv().await.unwrap(), "debug.pause");
        assert!(
            matches!(recv.read(&mut [0]).await, Err(iroh::endpoint::ReadError::Reset(code)) if code.into_inner() == 1)
        );
    }
    let mut held = Vec::new();
    for _ in 0..15 {
        held.push(raw(&f.connections[0], &request("hub.status", &f.hub, &f.device)).await);
        assert_eq!(f.entered.recv().await.unwrap(), "hub.status");
    }
    // Task1 is the device-side serve for connection0. Abort must drop its
    // JoinSet, resetting every operation and returning permits shared with1.
    f.tasks[1].abort();
    for mut recv in held {
        assert!(
            matches!(recv.read(&mut [0]).await, Err(iroh::endpoint::ReadError::Reset(code)) if code.into_inner() == 1)
        );
    }
    let mut held = Vec::new();
    for _ in 0..15 {
        held.push(raw(&f.connections[1], &request("hub.status", &f.hub, &f.device)).await);
        assert_eq!(f.entered.recv().await.unwrap(), "hub.status");
    }
    // Rejection must stop unread input with code1, not Iroh Drop's default0.
    let (mut send, mut recv) = f.connections[1].open_bi().await.unwrap();
    write_frame(&mut send, &request("hub.status", &f.hub, &f.device))
        .await
        .unwrap();
    assert_eq!(
        read_frame(&mut recv).await.unwrap()["error"]["code"],
        "resource_exhausted"
    );
    assert_eq!(send.stopped().await.unwrap().unwrap().into_inner(), 1);
    f.handler.release.add_permits(15);
    for mut recv in held {
        read_frame(&mut recv).await.unwrap();
    }
    f.close().await;
}

#[tokio::test]
async fn git_limit_is_shared_and_released_after_handler_deadline() {
    let mut f = Fixture::new().await;
    let mut value = request("hub.git.fetch", &f.hub, &f.device);
    value["params"] = json!({"ref":"refs/published/main"});
    value["timeout_ms"] = json!(300);
    let mut held = Vec::new();
    for i in 0..2 {
        held.push(raw(&f.connections[i], &value).await);
        assert_eq!(f.entered.recv().await.unwrap(), "hub.git.fetch");
    }
    assert_eq!(
        f.reply(0, &value).await["error"]["code"],
        "resource_exhausted"
    );
    for mut recv in held {
        assert!(
            matches!(recv.read(&mut [0]).await, Err(iroh::endpoint::ReadError::Reset(code)) if code.into_inner() == 1)
        );
    }
    let mut recovered = raw(&f.connections[1], &value).await;
    assert_eq!(f.entered.recv().await.unwrap(), "hub.git.fetch");
    f.handler.release.add_permits(1);
    assert_eq!(
        read_frame(&mut recovered).await.unwrap()["error"]["message"],
        "test handler reached"
    );
    f.close().await;
}

#[tokio::test]
async fn decoded_bytes_across_connections_reject_before_stream_limit_and_preserve_cancel() {
    let mut f = Fixture::new().await;
    let mut value = request("hub.events", &f.hub, &f.device);
    value.as_object_mut().unwrap().remove("context");
    value["params"] = json!({"offset":0,"events":[{"source":format!("coglet:{}",f.device.id()),"kind":"test.event","seq":0,"logical_time":0,"correlation_id":"aa".repeat(16),"payload":{"text":"x".repeat(500_000)}}]});
    let charge = serde_json::to_vec(&value).unwrap().len() * 2;
    let count = (coglet_proto::scheduler::MAX_QUEUED_BYTES
        - coglet_proto::scheduler::CONTROL_BYTES)
        / charge;
    assert!(count < 15 && count > 1);
    let mut held = Vec::new();
    for i in 0..count {
        held.push(raw(&f.reverse[i % 2], &value).await);
        assert_eq!(f.entered.recv().await.unwrap(), "hub.events");
    }
    let mut extra = raw(&f.reverse[1], &value).await;
    assert_eq!(
        read_frame(&mut extra).await.unwrap()["error"]["code"],
        "resource_exhausted"
    );
    // This endpoint is receiving the large events; send cancel to the same
    // endpoint, not the other direction's independent local scheduler.
    let cancel = request("hub.cancel", &f.hub, &f.device);
    let mut control = raw(&f.reverse[0], &cancel).await;
    assert_eq!(
        read_frame(&mut control).await.unwrap()["error"]["message"],
        "test handler reached"
    );
    assert_eq!(f.entered.recv().await.unwrap(), "hub.cancel");
    f.handler.release.add_permits(count);
    for mut recv in held {
        read_frame(&mut recv).await.unwrap();
    }
    f.close().await;
}

#[tokio::test]
async fn health_enroll_and_control_rates_are_separate_across_connections() {
    let mut f = Fixture::new().await;
    let health = json!({"method":"hub.health","correlation_id":"ab".repeat(16),"target":null,"timeout_ms":1000,"params":{"device":f.device.id().to_string(),"uptime_ms":0,"queue_depth":0,"available_bytes":0,"coglets":[]}});
    for i in 0..121 {
        let mut recv = raw(&f.reverse[i % 2], &health).await;
        let reply = read_frame(&mut recv).await.unwrap();
        if i < 120 {
            assert_eq!(reply["error"]["message"], "test handler reached");
            f.entered.recv().await.unwrap();
        } else {
            assert_eq!(reply["error"]["code"], "resource_exhausted");
        }
    }
    let cancel = request("hub.cancel", &f.hub, &f.device);
    let mut recv = raw(&f.reverse[0], &cancel).await;
    assert_eq!(
        read_frame(&mut recv).await.unwrap()["error"]["message"],
        "test handler reached"
    );
    f.entered.recv().await.unwrap();
    let enroll = json!({"method":"hub.enroll","correlation_id":"ef".repeat(16),"target":null,"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":null}});
    let mut recv = raw(&f.reverse[1], &enroll).await;
    assert_eq!(read_frame(&mut recv).await.unwrap()["type"], "reply");
    f.close().await;
}

#[tokio::test]
async fn one_peer_two_connections_bulk_saturation_preserves_stop_and_cancel() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let (tx, mut entered) = mpsc::unbounded_channel();
        let handler = Arc::new(Handler { entered: tx, release: Semaphore::new(0) });
        let mut connections = Vec::new();
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let h = hub.clone();
            let accept = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
            let dc = device.connect(hub.addr(), ALPN).await.unwrap();
            let hc = accept.await.unwrap();
            let hs = Session::new(hc.clone(), hub.id(), Role::Hub).unwrap();
            let ds = Session::new(dc.clone(), device.id(), Role::Device).unwrap();
            tasks.push(tokio::spawn(hs.serve(handler.clone())));
            let enroll = json!({"method":"hub.enroll","correlation_id":"ef".repeat(16),"target":null,"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":null}});
            assert_eq!(ds.request(enroll).await.unwrap()["type"], "reply");
            tasks.push(tokio::spawn(ds.serve(handler.clone())));
            connections.push(hc);
        }
        assert_ne!(connections[0].stable_id(), connections[1].stable_id());
        let mut held = Vec::new();
        for i in 0..15 {
            held.push(raw(&connections[i%2], &request("hub.status", &hub, &device)).await);
            assert_eq!(entered.recv().await.unwrap(), "hub.status");
        }
        let mut extra = raw(&connections[1], &request("hub.status", &hub, &device)).await;
        let rejected = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut extra)).await.expect("16th bulk must reject, not queue").unwrap();
        assert_eq!(rejected["error"]["code"], "resource_exhausted");
        assert_eq!(rejected["error"]["retryable"], true);
        for method in ["hub.stop", "hub.cancel"] {
            let mut control = raw(&connections[0], &request(method, &hub, &device)).await;
            let reply = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut control)).await.unwrap().unwrap();
            assert_eq!(reply["error"]["message"], "test handler reached");
            assert_eq!(entered.recv().await.unwrap(), method);
        }
        handler.release.add_permits(15);
        for mut recv in held { read_frame(&mut recv).await.unwrap(); }
        device.close().await; hub.close().await;
        for task in tasks { task.abort(); }
    }).await.unwrap();
}
