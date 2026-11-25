//! Real shared SQLite/Authority configuration seam, not deploy qualification.
use coglet_hub::{
    Kernel, Store,
    operator::{PeerIdentity, Request},
};
use coglet_proto::authority::{Authority, Grant};
use iroh::SecretKey;
use rusqlite::Connection;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[test]
fn operator_grant_configuration_intersects_enrollment_clock_and_generation() {
    use coglet_proto::authority::AuthorityError;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    let authority = Authority::open(&path).unwrap();
    let device = SecretKey::generate().public();
    let mut db = Connection::open(&path).unwrap();
    let mut grant = Grant {
        method: "hub.status".into(),
        target: None,
        expires: 100,
        generation: 1,
    };
    {
        let tx = db.transaction().unwrap();
        assert_eq!(
            Authority::configure_grant_in(&tx, device, &grant, Some(10)),
            Err(AuthorityError::Unauthenticated)
        );
    }
    authority.open_window(Duration::from_secs(60)).unwrap();
    let secret = authority
        .issue_invite(device, Duration::from_secs(60), vec![])
        .unwrap();
    authority.enroll(device, Some(&secret)).unwrap();
    {
        let tx = db.transaction().unwrap();
        assert_eq!(
            Authority::configure_grant_in(&tx, device, &grant, None),
            Err(AuthorityError::PermissionDenied)
        );
        assert_eq!(
            Authority::configure_grant_in(&tx, device, &grant, Some(100)),
            Err(AuthorityError::PermissionDenied)
        );
        Authority::configure_grant_in(&tx, device, &grant, Some(10)).unwrap();
        Authority::configure_grant_in(&tx, device, &grant, Some(10)).unwrap();
        grant.expires = 200;
        assert_eq!(
            Authority::configure_grant_in(&tx, device, &grant, Some(10)),
            Err(AuthorityError::PermissionDenied)
        );
        grant.generation = 0;
        assert_eq!(
            Authority::configure_grant_in(&tx, device, &grant, Some(10)),
            Err(AuthorityError::PermissionDenied)
        );
        grant.generation = 2;
        Authority::configure_grant_in(&tx, device, &grant, Some(10)).unwrap();
        tx.commit().unwrap();
    }
    assert!(
        authority
            .authorize(device, "hub.status", None, Some(10), 1)
            .is_err()
    );
    authority
        .authorize(device, "hub.status", None, Some(10), 2)
        .unwrap();
    let scope = coglet_proto::authority::Scope {
        actor: format!("coglet:{}", "00".repeat(32)),
        ticket: None,
    };
    assert_eq!(
        authority.authorize_scope(device, &scope, &grant, Some(10)),
        Err(AuthorityError::PermissionDenied),
        "peer grant must not create actor scope"
    );
    assert!(
        authority
            .authorize(device, "hub.stop", None, Some(10), 2)
            .is_err()
    );
}

#[test]
fn operator_configuration_rejects_legacy_split_state_without_touching_it() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("authority.db");
    std::fs::write(&legacy, b"retained legacy state").unwrap();
    let result = Kernel::open(
        dir.path(),
        SecretKey::generate().public(),
        Arc::new(|| Some(10)),
    );
    assert!(
        result.is_err(),
        "split authority must not be silently ignored"
    );
    assert_eq!(std::fs::read(legacy).unwrap(), b"retained legacy state");
    assert!(
        !dir.path().join("kernel.db").exists(),
        "reject before creating replacement state"
    );
}

#[test]
fn operator_grant_receipt_audit_and_authority_share_one_commit() {
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let _kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(10))).unwrap();
    let db_path = dir.path().join("kernel.db");
    let authority = Authority::open(&db_path).unwrap();
    let device = SecretKey::generate().public();
    authority.open_window(Duration::from_secs(60)).unwrap();
    let secret = authority
        .issue_invite(device, Duration::from_secs(60), vec![])
        .unwrap();
    authority.enroll(device, Some(&secret)).unwrap();
    let mut store = Store::open(&db_path).unwrap();
    assert!(!dir.path().join("authority.db").exists());
    let grant = Grant {
        method: "hub.status".into(),
        target: None,
        expires: 100,
        generation: 1,
    };
    let request = Request::parse(&serde_json::to_vec(&json!({
        "version":"local-1", "operation_id":"11".repeat(16), "op":"grant.provision", "timeout_ms":100,
        "params":{"peer":device.to_string(),"method":grant.method,"target":null,"generation":1,"expires_unix_s":100}
    })).unwrap()).unwrap();
    store
        .operator_admit(
            PeerIdentity {
                uid: 17,
                gid: None,
                pid: None,
            },
            &request,
        )
        .unwrap();
    let db = Connection::open(&db_path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_operator_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
    let finish = |store: &mut Store| {
        store.operator_finish(17, &request, None, |tx| {
            Authority::configure_grant_in(tx, device, &grant, Some(10))?;
            Ok(serde_json::Value::Null)
        })
    };
    assert!(finish(&mut store).is_err());
    assert!(
        authority
            .authorize(device, "hub.status", None, Some(10), 1)
            .is_err()
    );
    db.execute_batch("DROP TRIGGER fail_operator_audit")
        .unwrap();
    let applied = finish(&mut store).unwrap();
    assert_eq!(applied["receipt"]["state"], "applied");
    authority
        .authorize(device, "hub.status", None, Some(10), 1)
        .unwrap();
    drop(store);
    let mut store = Store::open(&db_path).unwrap();
    assert_eq!(
        store
            .operator_finish(17, &request, None, |_| panic!(
                "terminal retry must not write authority"
            ))
            .unwrap(),
        applied
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM proto_grants", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
