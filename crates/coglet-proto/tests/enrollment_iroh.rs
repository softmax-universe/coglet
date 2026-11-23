//! Exercises the reusable authority over real encrypted iroh, not fake replies.
use coglet_proto::{
    authority::{Authority, Grant},
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

struct Service(Authority);
impl OperationHandler for Service {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            let result = if request["method"] == "hub.enroll" {
                self.0.enroll(context.peer, request["params"]["invite"].as_str()).map(|_| json!({"selection":{"version":"0.1"},"hub":context.local.to_string(),"device":context.peer.to_string()}))
            } else {
                self.0
                    .authorize(
                        context.peer,
                        request["method"].as_str().unwrap(),
                        request["target"].as_str(),
                        Some(1),
                        1,
                    )
                    .map(|_| json!({}))
            };
            match result {
                Ok(result) => {
                    json!({"type":"reply","correlation_id":request["correlation_id"],"result":result})
                }
                Err(error) => {
                    json!({"type":"error","correlation_id":request["correlation_id"],"error":{"code":error.to_string(),"message":"rejected","retryable":false}})
                }
            }
        })
    }
}

#[tokio::test]
async fn enrollment_grants_and_live_revocation_on_real_iroh() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let path = std::env::temp_dir().join(format!("coglet-enroll-wire-{}.sqlite", SecretKey::generate().public()));
        let authority = Authority::open(&path).unwrap();
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        authority.open_window(Duration::from_secs(30)).unwrap();
        let secret = authority.issue_invite(device.id(), Duration::from_secs(30), vec![Grant{method:"hub.health".into(), target:None,expires:1000,generation:1}]).unwrap();
        let h = hub.clone(); let service = Arc::new(Service(authority.clone()));
        let serving = tokio::spawn(async move {
            let connection = h.accept().await.unwrap().await.unwrap();
            Session::new(connection, h.id(), Role::Hub).unwrap().serve(service).await
        });
        let connection = device.connect(hub.addr(), ALPN).await.unwrap();
        let session = Session::new(connection, device.id(), Role::Device).unwrap();
        let enroll = |secret:Option<&str>| json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":secret}});
        assert_eq!(session.request(enroll(None)).await.unwrap()["error"]["code"], "unauthenticated");
        assert_eq!(session.request(enroll(Some(&secret))).await.unwrap()["type"], "reply");
        let health = json!({"method":"hub.health","target":null,"correlation_id":"cd".repeat(16),"timeout_ms":1000,"params":{"device":device.id().to_string(),"uptime_ms":0,"queue_depth":0,"available_bytes":0,"coglets":[]}});
        assert_eq!(session.request(health.clone()).await.unwrap()["type"], "reply");
        authority.revoke_peer(device.id()).unwrap();
        assert_eq!(session.request(health).await.unwrap()["error"]["code"], "permission_denied");
        assert_eq!(session.request(enroll(None)).await.unwrap()["error"]["code"], "unauthenticated");
        serving.abort(); let _ = serving.await;
        device.close().await; hub.close().await;
        drop(authority); std::fs::remove_file(path).unwrap();
    }).await.unwrap();
}
