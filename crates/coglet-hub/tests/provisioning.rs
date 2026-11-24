use coglet_hub::{Kernel, Registration};
use coglet_proto::signing;
use iroh::SecretKey;
use rusqlite::Connection;
use serde_json::json;
use std::sync::Arc;

fn registration(hub: iroh::EndpointId) -> Registration {
    let key = SecretKey::generate();
    let device = SecretKey::generate().public();
    let coglet = format!("coglet:{}", key.public());
    let statement = json!({"purpose":"delegation","hub":hub.to_string(),"device":device.to_string(),"coglet":coglet,"generation":1,"expires":1000,"methods":["hub.events"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    Registration {
        coglet,
        device: device.to_string(),
        grant_generation: 0,
        delegation: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
        events: vec![],
        actions: vec![],
    }
}

#[tokio::test]
async fn ticket_end_is_durable_exact_scope_and_cancels_unattempted_actions() {
    use coglet_hub::ActionGrant;
    use coglet_proto::{
        authority::Grant,
        transport::{OperationHandler, RequestContext},
    };
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let mut r = registration(hub);
    let device = r.device.parse().unwrap();
    let actor = format!("hub:{hub}");
    let ticket = "aa".repeat(16);
    // Sign a fresh actions-enabled delegation.
    let key = SecretKey::generate();
    r.coglet = format!("coglet:{}", key.public());
    let statement = json!({"purpose":"delegation","hub":hub.to_string(),"device":r.device,"coglet":r.coglet,"generation":1,"expires":1000,"methods":["hub.actions"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    r.delegation = json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())});
    r.actions = vec![
        ActionGrant {
            actor: actor.clone(),
            ticket: Some(ticket.clone()),
            capability: "test.move".into(),
        },
        ActionGrant {
            actor: actor.clone(),
            ticket: None,
            capability: "test.move".into(),
        },
    ];
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = kernel
        .invite(
            device,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.actions".into(),
                target: Some(r.coglet.clone()),
                expires: 1000,
                generation: 0,
            }],
        )
        .await
        .unwrap();
    let enroll = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
    let ctx = RequestContext {
        peer: device,
        local: hub,
        deadline: tokio::time::Instant::now() + Duration::from_secs(1),
    };
    assert_eq!(kernel.handle(ctx, enroll).await["type"], "reply");
    kernel.register(r.clone()).await.unwrap();
    let action = json!({"id":"44".repeat(16),"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
    kernel
        .prepare_action(
            &r.coglet,
            action.clone(),
            json!({"actor":actor,"ticket":ticket}),
            Some(0),
        )
        .await
        .unwrap();
    kernel.end_ticket(&actor, &ticket).await.unwrap();
    let db = Connection::open(dir.path().join("kernel.db")).unwrap();
    let reply: String = db
        .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&reply).unwrap()["error"]["code"],
        "permission_denied"
    );
    drop(kernel);
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    assert!(
        kernel
            .prepare_action(
                &r.coglet,
                action.clone(),
                json!({"actor":actor,"ticket":ticket}),
                None
            )
            .await
            .is_err()
    );
    let mut other = action;
    other["id"] = json!("55".repeat(16));
    kernel
        .prepare_action(&r.coglet, other, json!({"actor":actor}), Some(0))
        .await
        .unwrap();
}

#[tokio::test]
async fn expired_pending_registration_requires_explicit_recovery_and_survives_another_crash() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let now = Arc::new(AtomicU64::new(1));
    let clock: Arc<dyn Fn() -> Option<u64> + Send + Sync> = {
        let now = now.clone();
        Arc::new(move || Some(now.load(Ordering::SeqCst)))
    };
    let kernel = Kernel::open(dir.path(), hub, clock.clone()).unwrap();
    let key = SecretKey::generate();
    let mut r = registration(hub);
    r.coglet = format!("coglet:{}", key.public());
    let proof = |generation, expires| {
        let statement = json!({"purpose":"delegation","hub":hub.to_string(),"device":r.device,"coglet":r.coglet,"generation":generation,"expires":expires,"methods":["hub.events"]});
        let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
        json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())})
    };
    let initial = proof(1, 10);
    let replacement = proof(2, 30);
    let renewed = proof(3, 1000);
    r.delegation = initial;
    let db = Connection::open(dir.path().join("kernel.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_registry BEFORE INSERT ON registry BEGIN SELECT RAISE(ABORT,'injected registry failure'); END;").unwrap();
    assert!(kernel.register(r.clone()).await.is_err());
    now.store(20, Ordering::SeqCst);
    assert!(kernel.register(r.clone()).await.is_err());
    let mut recovered = r.clone();
    recovered.delegation = replacement;
    assert!(
        kernel.register(recovered.clone()).await.is_err(),
        "signature is not operator replacement approval"
    );
    // Same generation and policy changes remain forbidden on the recovery API.
    assert!(kernel.recover_registration(r).await.is_err());
    let mut changed_policy = recovered.clone();
    changed_policy.grant_generation = 99;
    assert!(kernel.recover_registration(changed_policy).await.is_err());
    assert!(
        kernel
            .recover_registration(recovered.clone())
            .await
            .is_err()
    );
    drop(kernel);
    db.execute_batch("DROP TRIGGER fail_registry;").unwrap();
    let kernel = Kernel::open(dir.path(), hub, clock.clone()).unwrap();
    now.store(40, Ordering::SeqCst);
    assert!(
        kernel
            .recover_registration(recovered.clone())
            .await
            .is_err()
    );
    recovered.delegation = renewed;
    kernel
        .recover_registration(recovered.clone())
        .await
        .unwrap();
    drop(kernel);
    let kernel = Kernel::open(dir.path(), hub, clock).unwrap();
    kernel.register(recovered).await.unwrap();
    assert_eq!(
        kernel.status().await.unwrap()["coglets"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        kernel
            .audit()
            .await
            .unwrap()
            .iter()
            .any(|r| r["method"] == "operator.recover_registration" && r["outcome"] == "approved")
    );
}

#[tokio::test]
async fn private_speech_owner_cannot_be_widened_by_another_registration() {
    use coglet_hub::EventGrant;
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let source = format!("surface:{}", SecretKey::generate().public());
    let mut owner = registration(hub);
    owner.events = vec![EventGrant {
        source: source.clone(),
        kind: "speech.start".into(),
    }];
    kernel.register(owner.clone()).await.unwrap();
    let mut other = registration(hub);
    other.events = vec![EventGrant {
        source,
        kind: "speech.partial".into(),
    }];
    assert!(
        kernel.register(other).await.is_err(),
        "another coglet acquired a private speech source"
    );
    assert!(kernel.events(&owner.coglet).await.unwrap().is_empty());
}

#[tokio::test]
async fn malformed_delegation_does_not_reserve_provisioning_identity() {
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let r = registration(hub);
    let mut bad = r.clone();
    bad.delegation["signature"] = json!("00".repeat(64));
    assert!(kernel.register(bad).await.is_err());
    kernel.register(r).await.unwrap();
}

#[tokio::test]
async fn audit_failure_precedes_authority_and_registry_failure_has_durable_recovery_intent() {
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let db = Connection::open(dir.path().join("kernel.db")).unwrap();
    let auth = Connection::open(dir.path().join("authority.db")).unwrap();
    let r = registration(hub);
    db.execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
    assert!(kernel.register(r.clone()).await.is_err());
    assert_eq!(
        auth.query_row("SELECT count(*) FROM proto_delegations", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "authority changed without a durable audited provisioning intent"
    );
    db.execute_batch("DROP TRIGGER fail_audit; CREATE TRIGGER fail_registry BEFORE INSERT ON registry BEGIN SELECT RAISE(ABORT,'injected registry failure'); END;").unwrap();
    assert!(kernel.register(r.clone()).await.is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM provisioning", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        kernel.status().await.unwrap()["coglets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(kernel);
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let mut conflicting = r.clone();
    conflicting.grant_generation = 2;
    assert!(kernel.register(conflicting).await.is_err());
    db.execute_batch("DROP TRIGGER fail_registry;").unwrap();
    kernel.register(r.clone()).await.unwrap();
    kernel.register(r).await.unwrap();
    assert_eq!(
        kernel.status().await.unwrap()["coglets"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM provisioning", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
