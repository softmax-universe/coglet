use super::*;
use coglet_proto::authority::accepted_refs::PushOperation;

fn request(h: &Harness, bytes: &[u8], m: &VerifiedRef) -> serde_json::Value {
    json!({"method":"hub.git.push","target":h.coglet(),"context":{"actor":h.scope.actor,"ticket":h.scope.ticket},"correlation_id":"ef".repeat(16),"timeout_ms":5000,"params":{"update":m.signed(),"size":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))}})
}

#[tokio::test]
async fn operation_receipt_failure_and_final_expiry_roll_back_ref_audit_and_receipt() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let m = metadata(&bytes, 1, None);
    let r = request(&h, &bytes, &m);
    let op = PushOperation::new(h.peer, &r).unwrap();
    let db = Connection::open(&h.path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON proto_git_operations BEGIN SELECT RAISE(ABORT,'disk full'); END;").unwrap();
    assert!(
        h.store
            .accept_push(
                validated(&root, &bytes, &m).await,
                &h.context(Mode::Authoritative),
                &op,
                || Some(1)
            )
            .is_err()
    );
    assert!(h.current().is_none());
    assert_eq!(h.count("proto_ref_audit"), 0);
    assert_eq!(h.count("proto_git_operations"), 0);
    db.execute_batch("DROP TRIGGER fail_receipt;").unwrap();
    let calls = std::cell::Cell::new(0);
    assert!(
        h.store
            .accept_push(
                validated(&root, &bytes, &m).await,
                &h.context(Mode::Authoritative),
                &op,
                || {
                    let n = calls.get();
                    calls.set(n + 1);
                    Some(if n == 0 { 1 } else { 100 })
                }
            )
            .is_err()
    );
    assert!(h.current().is_none());
    assert_eq!(h.count("proto_ref_audit"), 0);
    assert_eq!(h.count("proto_git_operations"), 0);
    assert_eq!(
        h.store
            .push_receipt(&m, &h.context(Mode::Authoritative), &op, || Some(1))
            .unwrap(),
        None
    );
    let receipt = h
        .store
        .accept_push(
            validated(&root, &bytes, &m).await,
            &h.context(Mode::Authoritative),
            &op,
            || Some(1),
        )
        .unwrap();
    let other = Harness::open(&root.0);
    assert_eq!(
        other
            .store
            .push_receipt(&m, &other.context(Mode::Authoritative), &op, || Some(1))
            .unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        other
            .store
            .accept_push(
                validated(&root, &bytes, &m).await,
                &other.context(Mode::Authoritative),
                &op,
                || Some(1)
            )
            .unwrap(),
        receipt
    );
    for field in ["actor", "ticket"] {
        let mut changed = r.clone();
        changed["context"][field] = if field == "actor" {
            json!(format!("coglet:{}", SecretKey::from([5; 32]).public()))
        } else {
            json!("aa".repeat(16))
        };
        let changed_op = PushOperation::new(h.peer, &changed).unwrap();
        assert!(matches!(
            h.store
                .precheck_push(&m, &h.context(Mode::Authoritative), &changed_op, || Some(1)),
            Err(StoreError::Conflict)
        ));
    }
    let mut wrong = request(&h, &bytes, &m);
    wrong["params"]["sha256"] = json!("ab".repeat(32));
    assert!(matches!(
        h.store.precheck_push(
            &m,
            &h.context(Mode::Authoritative),
            &PushOperation::new(h.peer, &wrong).unwrap(),
            || Some(1)
        ),
        Err(StoreError::Conflict)
    ));
    h.authority.end_ticket(&h.scope).unwrap();
    assert!(
        h.store
            .precheck_push(&m, &h.context(Mode::Authoritative), &op, || Some(1))
            .is_err()
    );
    assert!(
        h.store
            .push_receipt(&m, &h.context(Mode::Authoritative), &op, || Some(1))
            .is_err()
    );
    assert!(
        h.store
            .accept_push(
                validated(&root, &bytes, &m).await,
                &h.context(Mode::Authoritative),
                &op,
                || Some(1)
            )
            .is_err()
    );
    assert_eq!(h.count("proto_ref_audit"), 1);
    assert_eq!(h.count("proto_git_operations"), 1);
}
