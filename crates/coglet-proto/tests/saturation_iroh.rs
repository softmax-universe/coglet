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
            if method == "hub.status" {
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
async fn raw(connection: &Connection, value: &Value) -> RecvStream {
    let (mut send, recv) = connection.open_bi().await.unwrap();
    write_frame(&mut send, value).await.unwrap();
    send.finish().unwrap();
    recv
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
