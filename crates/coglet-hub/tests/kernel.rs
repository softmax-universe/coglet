use coglet_hub::{EventGrant, Kernel, Registration};
use coglet_proto::{
    authority::Grant,
    signing,
    transport::{OperationHandler, RequestContext},
};
use iroh::SecretKey;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

fn proof(key: &SecretKey, hub: iroh::EndpointId, device: iroh::EndpointId) -> Value {
    let statement = json!({"purpose":"delegation","coglet":format!("coglet:{}",key.public()),"hub":hub.to_string(),"device":device.to_string(),"generation":9,"expires":1000,"methods":["hub.events","hub.actions"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())})
}
#[tokio::test]
async fn registered_binding_exact_source_policy_and_revocation_fence_events() {
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let key = SecretKey::generate();
    let coglet = format!("coglet:{}", key.public());
    let source = format!("surface:{}", SecretKey::generate().public());
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = kernel
        .invite(
            device,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.events".into(),
                target: Some(coglet.clone()),
                expires: 1000,
                generation: 3,
            }],
        )
        .await
        .unwrap();
    let ctx = || RequestContext {
        peer: device,
        local: hub,
        deadline: tokio::time::Instant::now() + Duration::from_secs(5),
    };
    let enrollment = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
    assert_eq!(kernel.handle(ctx(), enrollment).await["type"], "reply");
    kernel
        .register(Registration {
            coglet: coglet.clone(),
            device: device.to_string(),
            grant_generation: 3,
            delegation: proof(&key, hub, device),
            events: vec![EventGrant {
                source: source.clone(),
                kind: "test.tick".into(),
            }],
            actions: vec![],
        })
        .await
        .unwrap();
    let event = json!({"source":source,"kind":"test.tick","seq":2,"logical_time":7,"correlation_id":"22".repeat(16),"payload":{}});
    let mut request = json!({"method":"hub.events","target":coglet,"timeout_ms":1000,"correlation_id":"33".repeat(16),"params":{"offset":0,"events":[event]}});
    assert_eq!(
        kernel.handle(ctx(), request.clone()).await["result"]["next_offset"],
        1
    );
    request["params"]["events"][0]["kind"] = json!("runtime.timer");
    assert_eq!(
        kernel.handle(ctx(), request.clone()).await["error"]["code"],
        "permission_denied"
    );
    kernel.revoke_peer(device).await.unwrap();
    request["params"]["events"][0]["kind"] = json!("test.tick");
    assert_eq!(
        kernel.handle(ctx(), request).await["error"]["code"],
        "permission_denied"
    );
    let status = kernel.status().await.unwrap();
    assert_eq!(status["coglets"].as_array().unwrap().len(), 1);
    assert_eq!(kernel.events(&coglet).await.unwrap().len(), 1);
    let deliveries = kernel.event_deliveries(&coglet).await.unwrap();
    let foreign = format!("coglet:{}", SecretKey::generate().public());
    assert!(kernel.event_deliveries(&foreign).await.unwrap().is_empty());
    kernel.ack_events(&foreign, deliveries[0].0).await.unwrap();
    assert_eq!(kernel.event_deliveries(&coglet).await.unwrap(), deliveries);
    kernel.ack_events(&coglet, deliveries[0].0).await.unwrap();
    assert!(kernel.events(&coglet).await.unwrap().is_empty());
    let audit = kernel.audit().await.unwrap();
    assert!(audit.iter().any(|a| a["method"] == "hub.enroll"));
    assert!(!serde_json::to_string(&audit).unwrap().contains(&invite));
}
