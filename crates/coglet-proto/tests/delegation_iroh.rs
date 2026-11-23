//! Component integration, not the language-neutral conformance suite.
use coglet_proto::{
    authority::{Authority, Grant},
    signing,
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

struct Relay(Authority);
impl OperationHandler for Relay {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        let authority = self.0.clone();
        Box::pin(async move {
            // Component fixture has one live caller; production must bound this
            // blocking dispatch in the shared per-peer scheduler, still pending.
            tokio::task::spawn_blocking(move || {
                let method = request["method"].as_str().unwrap();
                let result = if method == "hub.enroll" {
                    authority.enroll(context.peer, request["params"]["invite"].as_str()).map(|_| json!({"selection":{"version":"0.1"},"hub":context.local.to_string(),"device":context.peer.to_string()}))
                } else {
                    let target = request["target"].as_str().unwrap();
                    authority.authorize(context.peer, method, Some(target), Some(1), 1)
                        .and_then(|_| authority.authorize_delegation(context.local, context.peer, target, method, || Some(1), 7))
                        .map(|_| json!({"next_offset":0}))
                };
                match result {
                    Ok(result) => json!({"type":"reply","correlation_id":request["correlation_id"],"result":result}),
                    Err(error) => json!({"type":"error","correlation_id":request["correlation_id"],"error":{"code":error.to_string(),"message":"rejected","retryable":false}}),
                }
            }).await.unwrap()
        })
    }
}

#[tokio::test]
async fn operator_replacement_fences_existing_real_iroh_session() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let authority = Authority::open(":memory:").unwrap();
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let key = SecretKey::generate();
        let coglet = format!("coglet:{}", key.public());
        let statement = json!({"purpose":"delegation","hub":hub.id().to_string(),"device":device.id().to_string(),"coglet":coglet,"generation":7,"expires":100,"methods":["hub.events"]});
        let sign = |statement:Value| { let signature = key.sign(&signing::preimage("delegation", &statement).unwrap()); json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}) };
        authority.accept_delegation(hub.id(), device.id(), &coglet, &sign(statement.clone()), || Some(1)).unwrap();
        authority.open_window(Duration::from_secs(60)).unwrap();
        // Enrollment generation is independent from the delegation generation.
        let invite = authority.issue_invite(device.id(), Duration::from_secs(60), vec![Grant{method:"hub.events".into(),target:Some(coglet.clone()),generation:1,expires:100}]).unwrap();
        let h = hub.clone();
        let relay = Arc::new(Relay(authority.clone()));
        let serving = tokio::spawn(async move {
            let connection = h.accept().await.unwrap().await.unwrap();
            Session::new(connection, h.id(), Role::Hub).unwrap().serve(relay).await
        });
        let connection = device.connect(hub.addr(), ALPN).await.unwrap();
        let session = Session::new(connection, device.id(), Role::Device).unwrap();
        let enroll = json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":invite}});
        assert_eq!(session.request(enroll).await.unwrap()["type"], "reply");
        let events = |id:&str| json!({"method":"hub.events","target":coglet,"correlation_id":id.repeat(16),"timeout_ms":1000,"params":{"offset":0,"events":[]}});
        assert_eq!(session.request(events("cd")).await.unwrap()["result"]["next_offset"], 0);
        let mut replacement = statement;
        let new_device = SecretKey::generate().public();
        replacement["device"] = json!(new_device.to_string());
        replacement["generation"] = json!(8);
        authority.replace_delegation(hub.id(), new_device, &coglet, &sign(replacement), || Some(1)).unwrap();
        // Connection remains enrolled and grant exists, but binding is fenced.
        authority.authorize(device.id(), "hub.events", Some(&coglet), Some(1), 1).unwrap();
        assert_eq!(session.request(events("ef")).await.unwrap()["error"]["code"], "permission_denied");
        serving.abort(); let _ = serving.await;
        device.close().await; hub.close().await;
    }).await.unwrap();
}
