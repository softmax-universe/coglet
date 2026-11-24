//! Component admission matrices. RequestContext is trusted local test input;
//! these do not certify transport authentication or the joined daemon route.
use coglet_hub::{ActionGrant, EventGrant, Kernel, Registration};
use coglet_proto::{
    authority::Grant,
    signing,
    transport::{OperationHandler, RequestContext},
};
use iroh::{EndpointId, SecretKey};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

struct Fixture {
    _dir: tempfile::TempDir,
    kernel: Kernel,
    hub: EndpointId,
    device: EndpointId,
    target: String,
    source: String,
    now: Arc<AtomicU64>,
}
impl Fixture {
    async fn new(kind: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let hub = SecretKey::generate().public();
        let device = SecretKey::generate().public();
        let key = SecretKey::generate();
        let target = format!("coglet:{}", key.public());
        let source = format!("surface:{}", SecretKey::generate().public());
        let now = Arc::new(AtomicU64::new(1));
        let clock = now.clone();
        let kernel = Kernel::open(
            dir.path(),
            hub,
            Arc::new(move || Some(clock.load(Ordering::SeqCst))),
        )
        .unwrap();
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite = kernel
            .invite(
                device,
                Duration::from_secs(60),
                ["hub.events", "hub.actions"]
                    .into_iter()
                    .map(|m| Grant {
                        method: m.into(),
                        target: Some(target.clone()),
                        expires: 1000,
                        generation: 3,
                    })
                    .collect(),
            )
            .await
            .unwrap();
        let statement = json!({"purpose":"delegation","coglet":target,"hub":hub.to_string(),"device":device.to_string(),"generation":9,"expires":1000,"methods":["hub.events","hub.actions"]});
        let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
        let fixture = Self {
            _dir: dir,
            kernel,
            hub,
            device,
            target,
            source,
            now,
        };
        let enrollment = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
        assert_eq!(
            fixture
                .kernel
                .handle(fixture.context(device), enrollment)
                .await["type"],
            "reply"
        );
        fixture.kernel.register(Registration {
            coglet: fixture.target.clone(), device: device.to_string(), grant_generation: 3,
            delegation: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
            events: vec![EventGrant { source: fixture.source.clone(), kind: kind.into() }],
            actions: vec![ActionGrant { actor: fixture.target.clone(), ticket: None, capability: "expression.set_expression".into() }],
        }).await.unwrap();
        fixture
    }
    fn context(&self, peer: EndpointId) -> RequestContext {
        RequestContext {
            peer,
            local: self.hub,
            deadline: tokio::time::Instant::now() + Duration::from_secs(5),
        }
    }
    fn events(&self, kind: &str) -> Value {
        json!({"method":"hub.events","target":self.target,"timeout_ms":1000,"correlation_id":"33".repeat(16),"params":{"offset":0,"events":[{"source":self.source,"kind":kind,"seq":2,"logical_time":7,"correlation_id":"22".repeat(16),"payload":{}}]}})
    }
    fn action(&self) -> Value {
        json!({"coglet":self.target,"id":"44".repeat(16),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":20000,"correlation_id":"55".repeat(16)})
    }
    fn actor(&self) -> Value {
        json!({"actor":self.target})
    }
}
#[tokio::test]
async fn relay_target_without_grant_denied_with_authorized_positive_control() {
    let f = Fixture::new("test.tick").await;
    let request = f.events("test.tick");
    let mut foreign = request.clone();
    foreign["target"] = json!(format!("coglet:{}", SecretKey::generate().public()));
    assert_eq!(
        f.kernel.handle(f.context(f.device), foreign).await["error"]["code"],
        "permission_denied"
    );
    assert_eq!(
        f.kernel
            .handle(f.context(SecretKey::generate().public()), request.clone())
            .await["error"]["code"],
        "permission_denied"
    );
    assert!(f.kernel.events(&f.target).await.unwrap().is_empty());
    assert_eq!(
        f.kernel.handle(f.context(f.device), request).await["result"]["next_offset"],
        1
    );
    assert_eq!(f.kernel.events(&f.target).await.unwrap().len(), 1);
}
#[tokio::test]
async fn relay_wrong_actor_and_ticket_denied_before_retention() {
    let f = Fixture::new("test.tick").await;
    let before = f.kernel.audit().await.unwrap();
    let wrong_actor = json!({"actor":format!("coglet:{}", SecretKey::generate().public())});
    assert!(
        f.kernel
            .prepare_action(&f.target, f.action(), wrong_actor, Some(100))
            .await
            .is_err()
    );
    let wrong_ticket = json!({"actor":f.target,"ticket":"aa".repeat(16)});
    assert!(
        f.kernel
            .prepare_action(&f.target, f.action(), wrong_ticket, Some(100))
            .await
            .is_err()
    );
    assert_eq!(f.kernel.audit().await.unwrap(), before);
    let accepted = f
        .kernel
        .prepare_action(&f.target, f.action(), f.actor(), Some(100))
        .await
        .unwrap();
    assert_eq!(accepted["params"]["action"]["deadline"], 20100);
    assert_eq!(
        f.kernel
            .prepare_action(&f.target, f.action(), f.actor(), Some(999))
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(f.kernel.audit().await.unwrap().len(), before.len() + 1);
}
#[tokio::test]
async fn relay_expiry_and_revocation_fence_identical_retries() {
    let f = Fixture::new("test.tick").await;
    let request = f.events("test.tick");
    assert_eq!(
        f.kernel.handle(f.context(f.device), request.clone()).await["type"],
        "reply"
    );
    f.now.store(1000, Ordering::SeqCst);
    assert_eq!(
        f.kernel.handle(f.context(f.device), request.clone()).await["error"]["code"],
        "permission_denied"
    );
    f.now.store(1, Ordering::SeqCst);
    assert_eq!(
        f.kernel.handle(f.context(f.device), request.clone()).await["result"]["next_offset"],
        1
    );
    f.kernel.revoke_peer(f.device).await.unwrap();
    assert_eq!(
        f.kernel.handle(f.context(f.device), request).await["error"]["code"],
        "permission_denied"
    );
    assert_eq!(f.kernel.events(&f.target).await.unwrap().len(), 1);
}
#[tokio::test]
async fn relay_private_owner_only_local_delivery_and_ack() {
    let f = Fixture::new("speech.partial").await;
    assert_eq!(
        f.kernel
            .handle(f.context(f.device), f.events("speech.partial"))
            .await["type"],
        "reply"
    );
    let deliveries = f.kernel.event_deliveries(&f.target).await.unwrap();
    assert_eq!(deliveries.len(), 1);
    let foreign = format!("coglet:{}", SecretKey::generate().public());
    assert!(
        f.kernel
            .event_deliveries(&foreign)
            .await
            .unwrap()
            .is_empty()
    );
    f.kernel
        .ack_events(&foreign, deliveries[0].0)
        .await
        .unwrap();
    assert_eq!(
        f.kernel.event_deliveries(&f.target).await.unwrap(),
        deliveries
    );
    f.kernel
        .ack_events(&f.target, deliveries[0].0)
        .await
        .unwrap();
    assert!(
        f.kernel
            .event_deliveries(&f.target)
            .await
            .unwrap()
            .is_empty()
    );
}
