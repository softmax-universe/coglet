use super::*;
use coglet_proto::authority::accepted_refs::PushOperation;

fn request(h: &Harness, bytes: &[u8], m: &VerifiedRef) -> serde_json::Value {
    json!({"method":"hub.git.push","target":h.coglet(),"context":{"actor":h.scope.actor,"ticket":h.scope.ticket},"correlation_id":"ef".repeat(16),"timeout_ms":5000,"params":{"update":m.signed(),"size":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))}})
}

#[tokio::test]
async fn installed_receipt_pressure_terminalizes_original_sql_id_after_failed_commit() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let source = Root::new();
    source.git(&["init", "--initial-branch=hub"]);
    fs::create_dir(source.0.join("bin")).unwrap();
    let wasm = b"\0asm\x01\0\0\0";
    fs::write(source.0.join("bin/step.wasm"), wasm).unwrap();
    let record = coglet_types::HubBuildRecord {
        version: "0.1".into(),
        source_commit: "ab".repeat(20),
        recipe_sha256: "11".repeat(32),
        policy_sha256: "22".repeat(32),
        transform_sha256: "33".repeat(32),
        original_sha256: hex::encode(Sha256::digest(wasm)),
        rewritten_sha256: hex::encode(Sha256::digest(wasm)),
    }
    .to_cbor()
    .unwrap();
    fs::create_dir(source.0.join("build")).unwrap();
    fs::write(source.0.join("build/provenance.cbor"), record).unwrap();
    source.git(&["add", "."]);
    source.git(&["commit", "-m", "code"]);
    let tip = String::from_utf8(source.git(&["rev-parse", "HEAD"])).unwrap();
    let archive = source.git(&["archive", "--format=tar", "HEAD"]);
    source.git(&["bundle", "create", "transfer.bundle", "refs/heads/hub"]);
    let bytes = fs::read(source.0.join("transfer.bundle")).unwrap();
    let key = SecretKey::from([1; 32]);
    let statement = json!({"purpose":"hub-ref","hub":h.hub.to_string(),"coglet":h.coglet(),"ref":"refs/heads/hub","old":null,"new":tip.trim(),"archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":1});
    let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("hub-ref", &statement).unwrap()).to_bytes()),"signer":key.public().to_string(),"statement":statement});
    let m = VerifiedRef::verify(&signed, RefKind::Hub, &h.hub.to_string(), &h.coglet()).unwrap();
    let grant = Grant {
        method: "hub.git.push".into(),
        target: Some(h.coglet()),
        expires: 100,
        generation: 7,
    };
    let invite = h
        .authority
        .issue_invite(h.hub, Duration::from_secs(30), vec![grant.clone()])
        .unwrap();
    h.authority.enroll(h.hub, Some(&invite)).unwrap();
    h.authority
        .provision_scope(h.hub, &h.scope, &grant)
        .unwrap();
    let context = Acceptance {
        hub: h.hub,
        coglet: h.coglet(),
        kind: RefKind::Hub,
        mode: Mode::Authoritative,
        peer: h.hub,
        scope: &h.scope,
        grant_generation: 7,
        delegation_generation: None,
    };
    let r = request(&h, &bytes, &m);
    let op = PushOperation::new(h.hub, &r).unwrap();
    h.store
        .accept_push(validated(&root, &bytes, &m).await, &context, &op, || {
            Some(1)
        })
        .unwrap();
    let db = Connection::open(&h.path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_terminal BEFORE UPDATE ON proto_git_operations BEGIN SELECT RAISE(ABORT,'disk full'); END;").unwrap();
    assert!(
        h.store
            .reconcile_installed_push(
                None,
                &context,
                &op,
                h.peer,
                0,
                || Some(1),
                |_| Err("resource_exhausted: lifecycle receipt ledger full".into())
            )
            .is_err()
    );
    assert_eq!(
        h.store
            .push_receipt(&m, &context, &op, || Some(1))
            .unwrap()
            .unwrap()["state"],
        "queued"
    );
    db.execute_batch("DROP TRIGGER fail_terminal;").unwrap();
    let terminal = h
        .store
        .reconcile_installed_push(
            None,
            &context,
            &op,
            h.peer,
            0,
            || Some(1),
            |_| Err("resource_exhausted: lifecycle receipt ledger full".into()),
        )
        .unwrap();
    assert_eq!(terminal["state"], "failed");
    assert_eq!(terminal["operation_id"], r["correlation_id"]);
    assert_eq!(terminal["error"]["code"], "resource_exhausted");
    let other = Harness::open(&root.0);
    assert_eq!(
        other
            .store
            .reconcile_installed_push(
                None,
                &context,
                &op,
                h.peer,
                0,
                || Some(1),
                |_| panic!("terminal SQL refusal must not retry application")
            )
            .unwrap(),
        terminal
    );
    let mut pending = true;
    other
        .store
        .sync_installed_push_fence(h.hub, &h.coglet(), |value| {
            pending = value;
            Ok(())
        })
        .unwrap();
    assert!(!pending);
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
