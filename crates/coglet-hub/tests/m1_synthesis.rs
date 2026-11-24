//! Router outcome authority over authenticated iroh sessions. The responder
//! acknowledges fixture actions; it does not claim a physical adapter effect.
use coglet_hub::{ActionGrant, Kernel, Registration};
use coglet_proto::{
    authority::Grant,
    signing,
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

struct Accept;
impl OperationHandler for Accept {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(request["method"], "hub.actions");
            json!({"type":"reply","correlation_id":request["correlation_id"],"result":{"id":request["params"]["action"]["id"],"correlation_id":request["params"]["action"]["correlation_id"],"accepted":true}})
        })
    }
}

async fn synthesis_matrix(status: &str) {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
        let key = SecretKey::generate();
        let target = format!("coglet:{}", key.public());
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite = kernel.invite(device.id(), Duration::from_secs(60), ["hub.events", "hub.actions"].into_iter().map(|method| Grant {
            method: method.into(), target: Some(target.clone()), expires: 1000, generation: 3,
        }).collect()).await.unwrap();
        let accepting = tokio::spawn({ let hub = hub.clone(); async move {
            Session::new(hub.accept().await.unwrap().await.unwrap(), hub.id(), Role::Hub).unwrap()
        }});
        let outgoing = Session::new(device.connect(hub.addr(), ALPN).await.unwrap(), device.id(), Role::Device).unwrap();
        let incoming = accepting.await.unwrap();
        let serving = tokio::spawn({let session = incoming.clone(); let kernel = kernel.clone(); async move {let _ = session.serve(Arc::new(kernel)).await;}});
        let enrolled = outgoing.request(json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}})).await.unwrap();
        assert_eq!(enrolled["type"], "reply");
        let statement = json!({"purpose":"delegation","coglet":target,"hub":hub.id().to_string(),"device":device.id().to_string(),"generation":9,"expires":1000,"methods":["hub.events","hub.actions"]});
        let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
        kernel.register(Registration {
            coglet: target.clone(), device: device.id().to_string(), grant_generation: 3,
            delegation: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
            events: vec![], actions: vec![ActionGrant {actor:target.clone(), ticket:None, capability:"expression.set_expression".into()}],
        }).await.unwrap();
        let action = json!({"coglet":target,"id":"44".repeat(16),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":20000,"correlation_id":"55".repeat(16)});
        let mut payload = json!({"id":action["id"],"correlation_id":action["correlation_id"],"status":status});
        if status == "succeeded" {payload["result"] = json!({});}
        else {payload["error"] = json!({"code":"unavailable","message":"fixture backend failure","retryable":false});}
        let event = json!({"source":target,"kind":"action.outcome","seq":1,"logical_time":7,"correlation_id":action["correlation_id"],"payload":payload});
        let request = json!({"method":"hub.events","target":target,"timeout_ms":1000,"correlation_id":"66".repeat(16),"params":{"offset":0,"events":[event]}});
        let before = kernel.audit().await.unwrap();
        assert_eq!(outgoing.request(request.clone()).await.unwrap()["error"]["code"], "permission_denied", "unknown action admitted");
        kernel.prepare_action(&target, action.clone(), json!({"actor":target}), Some(0)).await.unwrap();
        let retained = kernel.audit().await.unwrap();
        assert_eq!(retained.len(), before.len() + 1);
        assert_eq!(retained.last().unwrap()["method"], "hub.actions");
        assert_eq!(retained.last().unwrap()["operation_id"], action["id"]);
        assert_eq!(retained.last().unwrap()["actor"], target);
        assert_eq!(outgoing.request(request.clone()).await.unwrap()["error"]["code"], "permission_denied", "retention alone admitted success");
        assert_eq!(kernel.audit().await.unwrap(), retained);
        assert!(kernel.events(&target).await.unwrap().is_empty());
        let responder = tokio::spawn({let session = outgoing.clone(); async move {let _ = session.serve(Arc::new(Accept)).await;}});
        let accepted = kernel.dispatch_action_with_clock(&target, action["id"].as_str().unwrap(), incoming, Arc::new(||Some(1))).await.unwrap();
        assert_eq!(accepted["result"]["accepted"], true);
        let admitted = kernel.audit().await.unwrap();
        for field in ["source", "owner", "outer-correlation", "inner-correlation", "unknown-id"] {
            let mut forged = request.clone();
            match field {
                "source" => forged["params"]["events"][0]["source"] = json!(format!("surface:{}", SecretKey::generate().public())),
                "owner" => forged["target"] = json!(format!("coglet:{}", SecretKey::generate().public())),
                "outer-correlation" => forged["params"]["events"][0]["correlation_id"] = json!("aa".repeat(16)),
                "inner-correlation" => forged["params"]["events"][0]["payload"]["correlation_id"] = json!("aa".repeat(16)),
                "unknown-id" => forged["params"]["events"][0]["payload"]["id"] = json!("aa".repeat(16)),
                _ => unreachable!(),
            }
            assert_eq!(outgoing.request(forged).await.unwrap()["error"]["code"], "permission_denied", "accepted forged {field}");
            assert!(kernel.events(&target).await.unwrap().is_empty());
            assert_eq!(kernel.audit().await.unwrap(), admitted, "refusal changed retained authority audit");
        }
        assert_eq!(outgoing.request(request.clone()).await.unwrap()["result"]["next_offset"], 1);
        let terminal_audit = kernel.audit().await.unwrap();
        assert_eq!(terminal_audit.len(), admitted.len() + 1);
        let record = terminal_audit.last().unwrap();
        assert_eq!(record["method"], "hub.events");
        assert_eq!(record["peer"], device.id().to_string());
        assert_eq!(record["target"], target);
        assert_eq!(record["outcome"], "admitted");
        let delivery = kernel.event_deliveries(&target).await.unwrap();
        assert_eq!(delivery.len(), 1);
        assert_eq!(delivery[0].1["payload"], payload);
        assert_eq!(outgoing.request(request.clone()).await.unwrap()["result"]["next_offset"], 1);
        let mut duplicate = request.clone();
        duplicate["params"]["offset"] = json!(1);
        duplicate["params"]["events"][0]["seq"] = json!(2);
        assert_eq!(outgoing.request(duplicate.clone()).await.unwrap()["error"]["code"], "conflict");
        duplicate["params"]["events"][0]["payload"] = json!({"id":action["id"],"correlation_id":action["correlation_id"],"status":"cancelled","error":{"code":"cancelled","message":"fixture cancelled","retryable":false}});
        assert_eq!(outgoing.request(duplicate).await.unwrap()["error"]["code"], "conflict");
        assert_eq!(kernel.event_deliveries(&target).await.unwrap(), delivery);
        assert_eq!(kernel.audit().await.unwrap(), terminal_audit);
        responder.abort(); serving.abort(); device.close().await; hub.close().await;
        drop(kernel);
        let reopened = Kernel::open(dir.path(), hub.id(), Arc::new(||Some(1))).unwrap();
        assert_eq!(reopened.event_deliveries(&target).await.unwrap(), delivery);
        assert_eq!(reopened.audit().await.unwrap(), terminal_audit);
    }).await.expect("authenticated synthesis matrix exceeded watchdog");
}

#[tokio::test]
async fn synthesized_success_without_authority_denied_with_authenticated_positive_and_audit() {
    synthesis_matrix("succeeded").await;
}

#[tokio::test]
async fn authorized_terminal_failure_preserves_identity_dedup_and_audit() {
    synthesis_matrix("failed").await;
}
