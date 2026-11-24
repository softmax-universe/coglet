//! Operator-local installation proof only, not proposal or activation qualification.
//!
//! Initial API and repository guard tests were RED before implementation. The
//! transaction/CAS controls below are additional coverage of existing logic.
use coglet_hub::{Kernel, Registration};
use coglet_proto::{
    authority::Grant,
    signing,
    transport::{OperationHandler, RequestContext},
    wire,
};
use iroh::{EndpointId, SecretKey};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

async fn enrolled(root: &std::path::Path) -> (Kernel, EndpointId, EndpointId, String) {
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let key = SecretKey::generate();
    let target = format!("coglet:{}", key.public());
    let k = Kernel::open(root, hub, Arc::new(|| Some(1))).unwrap();
    k.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = k
        .invite(
            device,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.git.push".into(),
                target: Some(target.clone()),
                expires: 1000,
                generation: 3,
            }],
        )
        .await
        .unwrap();
    let context = RequestContext {
        peer: device,
        local: hub,
        deadline: tokio::time::Instant::now() + Duration::from_secs(5),
    };
    let request = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
    assert_eq!(k.handle(context, request).await["type"], "reply");
    let statement = json!({"purpose":"delegation","coglet":target,"hub":hub.to_string(),"device":device.to_string(),"generation":9,"expires":1000,"methods":["hub.git.push"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    k.register(Registration { coglet: target.clone(), device: device.to_string(), grant_generation: 3,
        delegation: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}), events:vec![], actions:vec![] }).await.unwrap();
    (k, hub, device, target)
}
#[tokio::test]
async fn installation_audit_failure_rolls_back_binding_revision_and_current() {
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, _, target) = enrolled(dir.path()).await;
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    let before = k.audit().await.unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'fault'); END;",
    )
    .unwrap();
    assert!(
        k.bind_policy_registry(&target, "application", "repo:game")
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM policy_registry", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_audit").unwrap();
    k.bind_policy_registry(&target, "application", "repo:game")
        .await
        .unwrap();
    let bound = k.audit().await.unwrap();
    assert_eq!(bound.len(), before.len() + 1);
    db.execute_batch(
        "CREATE TRIGGER fail_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'fault'); END;",
    )
    .unwrap();
    let bytes = wire::encode(&document(&target, &"a".repeat(32))).unwrap();
    assert!(k.install_policy(bytes.clone(), None).await.is_err());
    for table in ["policy_revisions", "policy_current"] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(k.audit().await.unwrap(), bound);
    drop(k);
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    db.execute_batch("DROP TRIGGER fail_audit").unwrap();
    k.install_policy(bytes.clone(), None).await.unwrap();
    k.install_policy(bytes, None).await.unwrap();
    assert_eq!(k.audit().await.unwrap().len(), bound.len() + 1);
}

#[tokio::test]
async fn concurrent_policy_replacement_has_one_cas_winner_and_no_revision_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, _, target) = enrolled(dir.path()).await;
    k.bind_policy_registry(&target, "application", "repo:game")
        .await
        .unwrap();
    let a = "a".repeat(32);
    let bytes = wire::encode(&document(&target, &a)).unwrap();
    k.install_policy(bytes.clone(), None).await.unwrap();
    let before = k.audit().await.unwrap();
    let b = wire::encode(&document(&target, &"b".repeat(32))).unwrap();
    let c = wire::encode(&document(&target, &"c".repeat(32))).unwrap();
    // Separate Kernel/SQLite connections exercise database CAS, not just a mutex.
    let other = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let (left, right) = tokio::join!(
        k.install_policy(b, Some(a.clone())),
        other.install_policy(c, Some(a.clone()))
    );
    assert_ne!(
        left.is_ok(),
        right.is_ok(),
        "exactly one replacement must win"
    );
    assert_eq!(k.audit().await.unwrap().len(), before.len() + 1);
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    let current: String = db
        .query_row(
            "SELECT revision FROM policy_current WHERE target=?1",
            [&target],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(current, a);
    assert_eq!(
        db.query_row("SELECT count(*) FROM policy_revisions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        k.install_policy(bytes.clone(), None).await.is_err(),
        "historical retry cannot reactivate"
    );
    assert!(
        k.install_policy(bytes, Some(current)).await.is_err(),
        "historical ID cannot be reinstalled"
    );
    let d = wire::encode(&document(&target, &"d".repeat(32))).unwrap();
    assert!(
        k.install_policy(d.clone(), None).await.is_err(),
        "None is not a wildcard"
    );
    assert!(
        k.install_policy(d, Some(a)).await.is_err(),
        "stale CAS cannot win"
    );
    assert!(
        db.execute("UPDATE policy_revisions SET document=x'00'", [])
            .is_err()
    );
    assert!(db.execute("DELETE FROM policy_registry", []).is_err());
    assert_eq!(k.audit().await.unwrap().len(), before.len() + 1);
}

fn document(target: &str, revision: &str) -> Value {
    json!({"version":"0.1","status":"DRAFT","coglet":target,"revision":revision,
        "control_role":"application","human_approval":false,
        "gates":[{"kind":"journal_replay","suite":"replay-v1","max_age_ms":100}],
        "probation":{"window_ms":1000,"health_interval_ms":10,"health_timeout_ms":100,
            "max_consecutive_failures":2,"max_step_failures":3,"max_budget_violations":1}})
}
#[tokio::test]
async fn registry_binding_rejects_credential_urls_before_reserving_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (k, _, _, target) = enrolled(dir.path()).await;
    for repo in [
        "https://user:secret@host/game",
        "repo:game?token=secret",
        "repo:game\n",
        "",
        "https://host/game",
    ] {
        assert!(
            k.bind_policy_registry(&target, "application", repo)
                .await
                .is_err(),
            "unsafe repo reference accepted: {repo:?}"
        );
    }
    assert!(
        k.bind_policy_registry(&target, "admin", "repo:game")
            .await
            .is_err()
    );
    k.bind_policy_registry(&target, "factory", "repo:game")
        .await
        .unwrap();
    let bytes = wire::encode(&document(&target, &"a".repeat(32))).unwrap();
    assert!(
        k.install_policy(bytes, None).await.is_err(),
        "document role cannot override operator registry"
    );
}

#[tokio::test]
async fn installation_pins_operator_role_and_exact_revision_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, device, target) = enrolled(dir.path()).await;
    let rev = "a".repeat(32);
    let bytes = wire::encode(&document(&target, &rev)).unwrap();
    assert!(
        k.install_policy(bytes.clone(), None).await.is_err(),
        "self-declared role is not registry authority"
    );
    k.bind_policy_registry(&target, "application", "repo:game")
        .await
        .unwrap();
    k.bind_policy_registry(&target, "application", "repo:game")
        .await
        .unwrap();
    assert!(
        k.bind_policy_registry(&target, "factory", "repo:game")
            .await
            .is_err()
    );
    assert!(
        k.bind_policy_registry(&target, "application", "repo:other")
            .await
            .is_err()
    );
    k.install_policy(bytes.clone(), None).await.unwrap();
    let before = k.audit().await.unwrap();
    drop(k);
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    k.install_policy(bytes.clone(), None).await.unwrap();
    assert_eq!(
        k.audit().await.unwrap(),
        before,
        "identical retry must not reinstall"
    );
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    let saved: Vec<u8> = db
        .query_row(
            "SELECT document FROM policy_revisions WHERE target=?1 AND revision=?2",
            rusqlite::params![target, rev],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saved, bytes);
    let mut changed = document(&target, &rev);
    changed["human_approval"] = json!(true);
    assert!(
        k.install_policy(wire::encode(&changed).unwrap(), Some(rev.clone()))
            .await
            .is_err()
    );
    assert_eq!(k.audit().await.unwrap(), before);
    k.revoke_peer(device).await.unwrap();
    assert!(
        k.install_policy(bytes, None).await.is_err(),
        "cached retries must recheck Authority"
    );
}
