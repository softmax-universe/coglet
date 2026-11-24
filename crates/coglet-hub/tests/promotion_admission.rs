//! Fixture authorities prove local storage admission, never native qualification.
use coglet_hub::kernel::promotion::{
    BuiltCandidate, GateAuthority, GateBinding, GateReport, PromotionAuthority, PromotionCaller,
    TicketLink,
};
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

struct ChangingAuthority(std::sync::atomic::AtomicUsize);
impl GateAuthority for ChangingAuthority {
    fn verify(&self, _: &GateReport, _: &GateBinding) -> coglet_hub::Result<()> {
        Ok(())
    }
}
impl PromotionAuthority for ChangingAuthority {
    fn now_ms(&self) -> coglet_hub::Result<u64> {
        Ok(10)
    }
    fn authorize(&self, _: &PromotionCaller, _: &str, _: &str) -> coglet_hub::Result<String> {
        Ok(format!(
            "grant-{}",
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
    }
    fn built_candidate(&self, b: &GateBinding) -> coglet_hub::Result<BuiltCandidate> {
        FixtureAuthority.built_candidate(b)
    }
    fn approval(
        &self,
        c: &PromotionCaller,
        b: &GateBinding,
        id: &str,
        r: &str,
    ) -> coglet_hub::Result<()> {
        FixtureAuthority.approval(c, b, id, r)
    }
    fn compatibility(&self, b: &GateBinding, built: &BuiltCandidate) -> coglet_hub::Result<()> {
        FixtureAuthority.compatibility(b, built)
    }
    fn ticket(
        &self,
        c: &PromotionCaller,
        b: &GateBinding,
        t: &TicketLink,
        v: &Value,
        built: &BuiltCandidate,
    ) -> coglet_hub::Result<coglet_hub::kernel::promotion::TicketRules> {
        FixtureAuthority.ticket(c, b, t, v, built)
    }
}
#[tokio::test]
async fn authority_cas_rejection_rolls_back_proposal_but_retains_receipt_and_audit() {
    let dir = tempfile::tempdir().unwrap();
    let (k, _, target) = setup(dir.path()).await;
    let authority = Arc::new(ChangingAuthority(std::sync::atomic::AtomicUsize::new(0)));
    let bytes = wire::encode(&propose(&target, "2")).unwrap();
    let reply = k
        .promotion_request(caller(), bytes, authority, None)
        .await
        .unwrap();
    assert_eq!(wire::decode(&reply).unwrap()["error"]["code"], "conflict");
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_proposals", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_receipts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        k.audit().await.unwrap().last().unwrap()["phase"],
        "rejected"
    );
    assert_eq!(
        request(&k, caller(), &propose(&target, "3")).await["result"]["state"],
        "proposed"
    );
}
#[tokio::test]
async fn promote_admits_one_exact_outbox_intent_without_claiming_activation() {
    use coglet_hub::kernel::promotion::{BoundaryIntent, PromotionBoundary};
    use sha2::{Digest, Sha256};
    struct FixtureBoundary(EndpointId);
    impl PromotionBoundary for FixtureBoundary {
        fn prepare(&self, b: &GateBinding, g: u64) -> coglet_hub::Result<BoundaryIntent> {
            let bundle = b"fixture bundle - not native qualification".to_vec();
            Ok(BoundaryIntent {
                statement: json!({"purpose":"hub-ref","hub":self.0.to_string(),"coglet":b.coglet,
                "ref":"refs/heads/hub","old":b.base,"new":b.candidate,"archive_sha256":hex::encode(Sha256::digest(&bundle)),"generation":g}),
                bundle,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, target) = setup(dir.path()).await;
    let p = request(&k, caller(), &propose(&target, "1")).await;
    let v = json!({"version":"0.1","op":"promote","correlation_id":"2".repeat(32),"coglet":target,
        "proposal_id":p["result"]["proposal_id"],"expected_generation":0});
    let input = wire::encode(&v).unwrap();
    let adapter = Arc::new(FixtureBoundary(hub));
    let fault_db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    let before = k.audit().await.unwrap();
    for table in ["promotion_outbox", "promotion_receipts", "audit"] {
        fault_db.execute_batch(&format!("CREATE TRIGGER fail_promote BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'fault'); END;")).unwrap();
        assert!(
            k.promotion_request(
                caller(),
                input.clone(),
                Arc::new(FixtureAuthority),
                Some(adapter.clone())
            )
            .await
            .is_err()
        );
        assert_eq!(k.audit().await.unwrap(), before);
        assert_eq!(
            fault_db
                .query_row("SELECT count(*) FROM promotion_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            fault_db
                .query_row("SELECT state FROM promotion_state", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "proposed"
        );
        let recovered = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
        assert_eq!(recovered.audit().await.unwrap(), before);
        fault_db.execute_batch("DROP TRIGGER fail_promote").unwrap();
    }
    let receipt = k
        .promotion_request(
            caller(),
            input.clone(),
            Arc::new(FixtureAuthority),
            Some(adapter.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        wire::decode(&receipt).unwrap()["result"]["state"],
        "activating"
    );
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let (live, g): (Option<String>, String) = db
        .query_row(
            "SELECT live_commit,generation FROM promotion_state",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(live, None);
    assert_eq!(g, "1");
    drop(k);
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    assert_eq!(
        k.promotion_request(caller(), input, Arc::new(FixtureAuthority), Some(adapter))
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[tokio::test]
async fn policy_replacement_invalidates_unactivated_proposal_and_audit_failure_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let (k, _, target) = setup(dir.path()).await;
    request(&k, caller(), &propose(&target, "1")).await;
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'fault'); END;",
    )
    .unwrap();
    let next = wire::encode(&policy(&target, "b")).unwrap();
    assert!(
        k.install_policy(next.clone(), Some("a".repeat(32)))
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row("SELECT state FROM promotion_proposals", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "proposed"
    );
    db.execute_batch("DROP TRIGGER fail_audit").unwrap();
    k.install_policy(next, Some("a".repeat(32))).await.unwrap();
    assert_eq!(
        db.query_row("SELECT state FROM promotion_proposals", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "invalidated"
    );
    let mut v = propose(&target, "2");
    v["policy_revision"] = json!("b".repeat(32));
    v["gates"][0]["policy_revision"] = json!("b".repeat(32));
    assert_eq!(
        request(&k, caller(), &v).await["result"]["state"],
        "proposed"
    );
}
struct ArtifactAuthority {
    compatible: bool,
    approved_id: String,
}
impl GateAuthority for ArtifactAuthority {
    fn verify(&self, r: &GateReport, b: &GateBinding) -> coglet_hub::Result<()> {
        FixtureAuthority.verify(r, b)
    }
}
impl PromotionAuthority for ArtifactAuthority {
    fn now_ms(&self) -> coglet_hub::Result<u64> {
        Ok(10)
    }
    fn authorize(&self, c: &PromotionCaller, t: &str, o: &str) -> coglet_hub::Result<String> {
        FixtureAuthority.authorize(c, t, o)
    }
    fn built_candidate(&self, b: &GateBinding) -> coglet_hub::Result<BuiltCandidate> {
        FixtureAuthority.built_candidate(b)
    }
    fn compatibility(&self, _: &GateBinding, _: &BuiltCandidate) -> coglet_hub::Result<()> {
        if self.compatible {
            Ok(())
        } else {
            Err(coglet_hub::Error::Denied)
        }
    }
    fn approval(
        &self,
        _: &PromotionCaller,
        _: &GateBinding,
        id: &str,
        _: &str,
    ) -> coglet_hub::Result<()> {
        if id == self.approved_id {
            Ok(())
        } else {
            Err(coglet_hub::Error::Denied)
        }
    }
    fn ticket(
        &self,
        _: &PromotionCaller,
        _: &GateBinding,
        t: &TicketLink,
        v: &Value,
        b: &BuiltCandidate,
    ) -> coglet_hub::Result<coglet_hub::kernel::promotion::TicketRules> {
        if t.evidence_ref != "artifact:done"
            || b.signature_ref != "artifact:build"
            || v["candidate"] != "b".repeat(40)
        {
            return Err(coglet_hub::Error::Denied);
        }
        Ok(coglet_hub::kernel::promotion::TicketRules {
            required: vec![coglet_hub::kernel::promotion::GateRequirement {
                kind: coglet_hub::kernel::promotion::GateKind::HeadlessGame,
                suite: "game-v1".into(),
                max_age_ms: 50,
            }],
            human_approval: true,
        })
    }
}
#[tokio::test]
async fn ticket_union_approval_request_binding_and_compatibility_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let (k, _, target) = setup(dir.path()).await;
    let mut c = caller();
    c.ticket = Some(TicketLink {
        ticket_id: "e".repeat(32),
        revision: 1,
        evidence_ref: "artifact:done".into(),
    });
    let mut v = propose(&target, "1");
    v["approval_ref"] = json!("artifact:approval");
    let mut game = v["gates"][0].clone();
    game["kind"] = json!("headless_game");
    game["suite"] = json!("game-v1");
    v["gates"].as_array_mut().unwrap().push(game);
    let run = |v: Value, c: PromotionCaller, compatible: bool| {
        let k = k.clone();
        async move {
            wire::decode(
                &k.promotion_request(
                    c,
                    wire::encode(&v).unwrap(),
                    Arc::new(ArtifactAuthority {
                        compatible,
                        approved_id: "3".repeat(32),
                    }),
                    None,
                )
                .await
                .unwrap(),
            )
            .unwrap()
        }
    };
    assert_eq!(
        run(v.clone(), c.clone(), true).await["error"]["code"],
        "permission_denied"
    );
    v["correlation_id"] = json!("2".repeat(32));
    assert_eq!(
        run(v.clone(), c.clone(), false).await["error"]["code"],
        "permission_denied"
    );
    v["correlation_id"] = json!("3".repeat(32));
    assert_eq!(run(v, c, true).await["result"]["state"], "proposed");
}
#[tokio::test]
async fn admitted_audit_links_verified_build_gate_and_ticket_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let (k, _, target) = setup(dir.path()).await;
    request(&k, caller(), &propose(&target, "1")).await;
    let audit = k.audit().await.unwrap();
    let record = audit.last().unwrap();
    assert_eq!(record["policy_revision"], "a".repeat(32));
    assert_eq!(
        record["evidence_refs"],
        json!(["artifact:build", "artifact:replay"])
    );
    assert_eq!(record["proposal_id"].as_str().unwrap().len(), 32);
}
#[tokio::test]
async fn proposal_faults_leave_no_partial_receipt_or_transition_after_restart() {
    for table in ["promotion_proposals", "promotion_receipts", "audit"] {
        let dir = tempfile::tempdir().unwrap();
        let (k, hub, target) = setup(dir.path()).await;
        let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
        let before = k.audit().await.unwrap();
        db.execute_batch(&format!("CREATE TRIGGER injected BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected'); END;")).unwrap();
        let input = wire::encode(&propose(&target, "1")).unwrap();
        assert!(
            k.promotion_request(caller(), input.clone(), Arc::new(FixtureAuthority), None)
                .await
                .is_err()
        );
        drop(k);
        let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
        for retained in [
            "promotion_proposals",
            "promotion_receipts",
            "promotion_state",
            "promotion_outbox",
        ] {
            assert_eq!(
                db.query_row(&format!("SELECT count(*) FROM {retained}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0,
                "fault={table}, retained={retained}"
            );
        }
        assert_eq!(k.audit().await.unwrap(), before);
        db.execute_batch("DROP TRIGGER injected").unwrap();
        let receipt = k
            .promotion_request(caller(), input.clone(), Arc::new(FixtureAuthority), None)
            .await
            .unwrap();
        let stored: (Vec<u8>, Vec<u8>) = db
            .query_row("SELECT input,response FROM promotion_receipts", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(stored, (input.clone(), receipt.clone()));
        assert_eq!(
            k.promotion_request(caller(), input, Arc::new(FixtureAuthority), None)
                .await
                .unwrap(),
            receipt
        );
    }
}
#[tokio::test]
async fn independent_sqlite_connections_serialize_competing_and_identical_proposals() {
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, target) = setup(dir.path()).await;
    let other = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let input = wire::encode(&propose(&target, "1")).unwrap();
    let (a, b) = tokio::join!(
        k.promotion_request(caller(), input.clone(), Arc::new(FixtureAuthority), None),
        other.promotion_request(caller(), input, Arc::new(FixtureAuthority), None)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    let mut c = caller();
    c.actor = format!("coglet:{}", "2".repeat(64));
    assert_eq!(
        request(&other, c, &propose(&target, "1")).await["error"]["code"],
        "conflict"
    );
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_receipts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_proposals", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let mut c = caller();
    c.ticket = Some(TicketLink {
        ticket_id: "e".repeat(32),
        revision: 1,
        evidence_ref: "artifact:other".into(),
    });
    assert_eq!(
        request(&k, c, &propose(&target, "1")).await["error"]["code"],
        "conflict"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_receipts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
struct RevocableArtifacts(Arc<std::sync::atomic::AtomicBool>);
impl GateAuthority for RevocableArtifacts {
    fn verify(&self, _: &GateReport, _: &GateBinding) -> coglet_hub::Result<()> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(coglet_hub::Error::Denied)
        }
    }
}
impl PromotionAuthority for RevocableArtifacts {
    fn now_ms(&self) -> coglet_hub::Result<u64> {
        Ok(10)
    }
    fn authorize(&self, c: &PromotionCaller, t: &str, o: &str) -> coglet_hub::Result<String> {
        FixtureAuthority.authorize(c, t, o)
    }
    fn built_candidate(&self, b: &GateBinding) -> coglet_hub::Result<BuiltCandidate> {
        FixtureAuthority.built_candidate(b)
    }
    fn compatibility(&self, b: &GateBinding, c: &BuiltCandidate) -> coglet_hub::Result<()> {
        FixtureAuthority.compatibility(b, c)
    }
    fn approval(
        &self,
        c: &PromotionCaller,
        b: &GateBinding,
        id: &str,
        r: &str,
    ) -> coglet_hub::Result<()> {
        FixtureAuthority.approval(c, b, id, r)
    }
    fn ticket(
        &self,
        c: &PromotionCaller,
        b: &GateBinding,
        t: &TicketLink,
        v: &Value,
        built: &BuiltCandidate,
    ) -> coglet_hub::Result<coglet_hub::kernel::promotion::TicketRules> {
        FixtureAuthority.ticket(c, b, t, v, built)
    }
}
#[tokio::test]
async fn artifact_revocation_during_boundary_preparation_rejects_without_outbox() {
    use coglet_hub::kernel::promotion::{BoundaryIntent, PromotionBoundary};
    use sha2::{Digest, Sha256};
    struct Revoke(EndpointId, Arc<std::sync::atomic::AtomicBool>);
    impl PromotionBoundary for Revoke {
        fn prepare(&self, b: &GateBinding, g: u64) -> coglet_hub::Result<BoundaryIntent> {
            self.1.store(false, std::sync::atomic::Ordering::SeqCst);
            let bundle = vec![1, 2, 3];
            Ok(BoundaryIntent {
                statement: json!({"purpose":"hub-ref","hub":self.0.to_string(),"coglet":b.coglet,
                "ref":"refs/heads/hub","old":b.base,"new":b.candidate,"archive_sha256":hex::encode(Sha256::digest(&bundle)),"generation":g}),
                bundle,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, target) = setup(dir.path()).await;
    let p = request(&k, caller(), &propose(&target, "1")).await;
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let v = json!({"version":"0.1","op":"promote","correlation_id":"2".repeat(32),"coglet":target,
        "proposal_id":p["result"]["proposal_id"],"expected_generation":0});
    let reply = k
        .promotion_request(
            caller(),
            wire::encode(&v).unwrap(),
            Arc::new(RevocableArtifacts(flag.clone())),
            Some(Arc::new(Revoke(hub, flag))),
        )
        .await
        .unwrap();
    assert_eq!(
        wire::decode(&reply).unwrap()["error"]["code"],
        "permission_denied"
    );
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM promotion_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT state FROM promotion_state", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "proposed"
    );
}
struct FixtureAuthority;
impl GateAuthority for FixtureAuthority {
    fn verify(&self, _: &GateReport, _: &GateBinding) -> coglet_hub::Result<()> {
        Ok(())
    }
}
impl PromotionAuthority for FixtureAuthority {
    fn now_ms(&self) -> coglet_hub::Result<u64> {
        Ok(10)
    }
    fn authorize(&self, _: &PromotionCaller, _: &str, _: &str) -> coglet_hub::Result<String> {
        Ok("grant-v1".into())
    }
    fn built_candidate(&self, _: &GateBinding) -> coglet_hub::Result<BuiltCandidate> {
        Ok(BuiltCandidate {
            wasm_sha256: "f".repeat(64),
            signature_ref: "artifact:build".into(),
        })
    }
    fn approval(
        &self,
        _: &PromotionCaller,
        _: &GateBinding,
        _: &str,
        _: &str,
    ) -> coglet_hub::Result<()> {
        Err(coglet_hub::Error::Denied)
    }
    fn compatibility(&self, _: &GateBinding, _: &BuiltCandidate) -> coglet_hub::Result<()> {
        Ok(())
    }
    fn ticket(
        &self,
        _: &PromotionCaller,
        _: &GateBinding,
        _: &TicketLink,
        _: &Value,
        _: &BuiltCandidate,
    ) -> coglet_hub::Result<coglet_hub::kernel::promotion::TicketRules> {
        Ok(coglet_hub::kernel::promotion::TicketRules {
            required: vec![],
            human_approval: false,
        })
    }
}
async fn setup(root: &std::path::Path) -> (Kernel, EndpointId, String) {
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
    let reply = k
        .handle(
            context,
            json!({"method":"hub.enroll","target":null,"timeout_ms":1000,
        "correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}}),
        )
        .await;
    assert_eq!(reply["type"], "reply");
    let statement = json!({"purpose":"delegation","coglet":target,"hub":hub.to_string(),"device":device.to_string(),
        "generation":9,"expires":1000,"methods":["hub.git.push"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    k.register(Registration { coglet:target.clone(), device:device.to_string(), grant_generation:3,
        delegation:json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
        events:vec![], actions:vec![] }).await.unwrap();
    k.bind_policy_registry(&target, "application", "repo:game")
        .await
        .unwrap();
    k.install_policy(wire::encode(&policy(&target, "a")).unwrap(), None)
        .await
        .unwrap();
    (k, hub, target)
}
fn policy(target: &str, revision: &str) -> Value {
    json!({"version":"0.1","status":"DRAFT","coglet":target,"revision":revision.repeat(32),
        "control_role":"application","human_approval":false,
        "gates":[{"kind":"journal_replay","suite":"replay-v1","max_age_ms":100}],
        "probation":{"window_ms":1000,"health_interval_ms":10,"health_timeout_ms":100,
            "max_consecutive_failures":2,"max_step_failures":3,"max_budget_violations":1}})
}
fn caller() -> PromotionCaller {
    PromotionCaller {
        actor: format!("coglet:{}", "1".repeat(64)),
        ticket: None,
    }
}
fn propose(target: &str, id: &str) -> Value {
    json!({"version":"0.1","correlation_id":id.repeat(32),"op":"propose","coglet":target,
        "candidate":"b".repeat(40),"base":null,"expected_generation":0,"policy_revision":"a".repeat(32),
        "gates":[{"kind":"journal_replay","suite":"replay-v1","candidate":"b".repeat(40),"base":null,
            "policy_revision":"a".repeat(32),"checked_at_ms":1,"status":"passed","evidence_ref":"artifact:replay"}]})
}
async fn request(k: &Kernel, c: PromotionCaller, v: &Value) -> Value {
    wire::decode(
        &k.promotion_request(
            c,
            wire::encode(v).unwrap(),
            Arc::new(FixtureAuthority),
            None,
        )
        .await
        .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn proposal_receipt_is_exact_and_survives_restart_without_new_audit() {
    let dir = tempfile::tempdir().unwrap();
    let (k, hub, target) = setup(dir.path()).await;
    let input = propose(&target, "1");
    let result = request(&k, caller(), &input).await;
    assert_eq!(result["result"]["state"], "proposed");
    let before = k.audit().await.unwrap();
    assert_eq!(request(&k, caller(), &input).await, result);
    assert_eq!(k.audit().await.unwrap(), before);
    drop(k);
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    assert_eq!(request(&k, caller(), &input).await, result);
    let mut conflicting = input.clone();
    conflicting["candidate"] = json!("c".repeat(40));
    assert_eq!(
        request(&k, caller(), &conflicting).await["error"]["code"],
        "conflict"
    );
    assert_eq!(request(&k, caller(), &input).await, result);
    assert_eq!(k.audit().await.unwrap(), before);
}
