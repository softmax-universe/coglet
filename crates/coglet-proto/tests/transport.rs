use coglet_proto::transport::{ALPN, OperationHandler, RequestContext, Role, Session};
use iroh::{Endpoint, endpoint::presets};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};

struct Handler;
struct ForgedEnrollment;
impl OperationHandler for ForgedEnrollment {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            json!({"type":"reply", "correlation_id":request["correlation_id"],"result":{"selection":{"version":"0.1"},"device":"00".repeat(32),"hub":"00".repeat(32)}})
        })
    }
}

#[tokio::test]
async fn rejects_enrollment_identity_substitution() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let h = hub.clone();
        let serving = tokio::spawn(async move {
            let connection = h.accept().await.unwrap().await.unwrap();
            Session::new(connection, h.id(), Role::Hub).unwrap().serve(Arc::new(ForgedEnrollment)).await
        });
        let connection = device.connect(hub.addr(), ALPN).await.unwrap();
        let session = Session::new(connection, device.id(), Role::Device).unwrap();
        let request = json!({"method":"hub.enroll","correlation_id":"cd".repeat(16),"target":null,"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":null}});
        assert!(session.request(request).await.is_err());
        device.close().await; hub.close().await; serving.abort();
    }).await.unwrap();
}
impl OperationHandler for Handler {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            assert!(!context.peer.to_string().is_empty());
            let result = if request["method"] == "hub.enroll" {
                json!({"selection":{"version":"0.1"},"device": context.peer.to_string(),"hub":context.local.to_string()})
            } else if request["method"] == "hub.status" {
                json!({"state":"stopped","live_commit":null,"step":0,"pending":0,"error":null})
            } else {
                json!({})
            };
            json!({"type":"reply", "correlation_id":request["correlation_id"],"result": result})
        })
    }
}

#[tokio::test]
async fn real_encrypted_connection_negotiates_then_carries_reverse_operation() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let hub_for_accept = hub.clone();
        let accept = tokio::spawn(async move {
            let connection = hub_for_accept.accept().await.unwrap().await.unwrap();
            Session::new(connection, hub_for_accept.id(), Role::Hub).unwrap()
        });
        let connection = device.connect(hub.addr(), ALPN).await.unwrap();
        let device_session = Session::new(connection, device.id(), Role::Device).unwrap();
        let hub_session = accept.await.unwrap();
        assert_eq!(hub_session.peer(), device.id());
        assert_eq!(device_session.peer(), hub.id());
        let hub_serve = tokio::spawn(hub_session.clone().serve(Arc::new(Handler)));
        let device_serve = tokio::spawn(device_session.clone().serve(Arc::new(Handler)));
        let health = json!({"method":"hub.health","correlation_id":"ab".repeat(16),"target":null,"timeout_ms":1000,"params":{"device":device.id().to_string(),"uptime_ms":0,"queue_depth":0,"available_bytes":0,"coglets":[]}});
        assert_eq!(device_session.request(health.clone()).await.unwrap()["error"]["code"], "unauthenticated");
        let enroll = json!({"method":"hub.enroll","correlation_id":"cd".repeat(16),"target":null,"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":null}});
        let reply = device_session.request(enroll).await.unwrap();
        assert_eq!(reply["result"]["selection"]["version"], "0.1");
        assert_eq!(device_session.request(health.clone()).await.unwrap()["type"], "reply");
        // The accepted connection opens a new stream back to the dialer; no inbound device listener.
        let status = json!({"method":"hub.status","correlation_id":"ef".repeat(16),"target":format!("coglet:{}",device.id()),"timeout_ms":1000,"params":{},"context":{"actor":format!("hub:{}",hub.id())}});
        assert_eq!(hub_session.request(status).await.unwrap()["result"]["state"], "stopped");
        // Health is device -> hub only, even though either endpoint can open streams.
        assert!(hub_session.request(health).await.is_err());
        device.close().await;
        hub.close().await;
        hub_serve.abort(); device_serve.abort();
    }).await.unwrap();
}
