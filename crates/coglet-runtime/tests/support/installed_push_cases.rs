//! Installed-update cases using the production router receiver and real bundles.
use super::*;
#[cfg(not(feature = "native-engine"))]
use coglet_runtime::MockEngine;
use coglet_runtime::{Engine, EngineReport, StepHost};

fn options() -> Options {
    let mut options = Options::receipts();
    options.delegation_methods.push("hub.git.push");
    options
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_push_drains_more_than_one_recovery_batch() {
    timed!(async {
        let mut env = Env::new(0x88, options()).await.unwrap();
        let (initial, _) = pushed_and_installed(&mut env).await;
        let barrier = InitialBoundaryBarrier::install(&mut env).await;
        let bundle = env.bundle(2, Some(&initial.tip), &[]);
        // Revalidating 131 real bundles is intentionally larger than a normal
        // request. Keep its wire deadline within the supported 300s limit and
        // the enclosing 120s test bound; do not alter production timeouts.
        let (id, pushing) = begin_push_with_timeout(&mut env, &bundle, 110_000);
        barrier.wait().await;
        // Test-local recovery backlog: every row retains a genuine verified
        // bundle and original intent; distinct IDs must get their own terminal
        // disposition, never vanish behind the first query's LIMIT 128.
        let db = rusqlite::Connection::open(env.root.join("authority.sqlite")).unwrap();
        for n in 0..130 {
            let child = correlation(500_000 + n);
            db.execute("INSERT INTO proto_git_operations SELECT peer,?1,target,digest,json_set(receipt,'$.operation_id',?1),audit_id FROM proto_git_operations WHERE operation_id=?2", rusqlite::params![child,id]).unwrap();
            db.execute("INSERT INTO proto_git_intents SELECT peer,?1,target,json_set(request,'$.correlation_id',?1) FROM proto_git_intents WHERE operation_id=?2", rusqlite::params![child,id]).unwrap();
        }
        barrier.release();
        assert_eq!(pushing.await.unwrap()["result"]["state"], "applied");
        let queued: i64 = db.query_row("SELECT count(*) FROM proto_git_operations WHERE json_extract(receipt,'$.state')='queued'", [], |r| r.get(0)).unwrap();
        assert_eq!(queued, 0);
        let failed: i64 = db.query_row("SELECT count(*) FROM proto_git_operations WHERE json_extract(receipt,'$.state')='failed'", [], |r| r.get(0)).unwrap();
        assert_eq!(failed, 130);
    });
}

fn context() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../../conformance/abi/cases.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone()
}
struct BlockAfter<E> {
    engine: E,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
}
impl<E: Engine> Engine for BlockAfter<E> {
    fn metadata(&self) -> Option<Value> {
        self.engine.metadata()
    }
    fn admit(&mut self, input: &Value, host: &StepHost) -> coglet_runtime::Result<()> {
        self.engine.admit(input, host)
    }
    fn execute(
        &mut self,
        input: &Value,
        host: &mut StepHost,
    ) -> coglet_runtime::Result<EngineReport> {
        let report = self.engine.execute(input, host)?;
        self.entered.take().unwrap().send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(15)).unwrap();
        Ok(report)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_push_applies_at_step_boundary() {
    timed!(async {
        let mut env = Env::new(0x81, options()).await.unwrap();
        let (initial, _) = pushed_and_installed(&mut env).await;
        if NATIVE {
            applied(&env.simple("hub.start").await);
        }
        let service = env
            .handler
            .as_ref()
            .unwrap()
            .share_service(&env.coglet)
            .unwrap();
        let mut fixture = context();
        {
            let mut locked = service.lock().unwrap();
            if !NATIVE {
                locked.managed_start().unwrap();
            }
            fixture["inputs"]["budgets"] = locked.managed_setup()["manifest"]["budgets"].clone();
            locked
                .enqueue(fixture["inputs"]["events"][0].clone())
                .unwrap();
        }
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let shared = service.clone();
        let old_step = tokio::task::spawn_blocking(move || {
            let mut locked = shared.lock().unwrap();
            #[cfg(not(feature = "native-engine"))]
            let engine = MockEngine::new(fixture["guest_script"].clone());
            #[cfg(feature = "native-engine")]
            let engine = coglet_runtime::NativeEngine::new(
                wat::parse_str(WASM_WAT).unwrap(),
                &fixture["inputs"]["budgets"],
                8_388_608,
                65_536,
            )
            .unwrap();
            locked
                .run_next(
                    &fixture["inputs"],
                    &mut BlockAfter {
                        engine,
                        entered: Some(entered_tx),
                        release: release_rx,
                    },
                )
                .unwrap()
                .unwrap()["journal"]
                .clone()
        });
        entered_rx.await.unwrap();
        let bundle = env.bundle(2, Some(&initial.tip), &[("revision", b"new code")]);
        let samples = env.reads.load(Ordering::SeqCst);
        let (id, push) = begin_push(&mut env, &bundle);
        tokio::time::timeout(Duration::from_secs(10), async {
            while env.reads.load(Ordering::SeqCst) == samples {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !push.is_finished(),
            "push cannot cross the running step mutex"
        );
        release_tx.send(()).unwrap();
        let journal = old_step.await.unwrap();
        assert_eq!(
            journal["code_hash"],
            hex::encode(Sha256::digest(wat::parse_str(WASM_WAT).unwrap()))
        );
        #[cfg(feature = "native-engine")]
        assert!(
            journal["usage"]["compute"].as_u64().unwrap() > 0,
            "actual WAMR step executed"
        );
        assert_eq!(push.await.unwrap()["result"]["state"], "applied");
        let terminal = result_of(&receipt_of(&mut env, &id).await);
        assert_eq!(terminal["state"], "applied", "{terminal}");
        assert_eq!(state(&env.simple("hub.status").await).1, bundle.tip);
        assert_eq!(service.lock().unwrap().records(), &[journal]);
        drop(service);
        // ADR-0027 recovery must revalidate the live revision, not install-time
        // bytes, while retaining the original authenticated start authority.
        if NATIVE {
            env.restart_handler().await;
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("running".into(), json!(bundle.tip))
            );
            assert_eq!(result_of(&receipt_of(&mut env, &id).await), terminal);
        }
        applied(&env.simple("hub.stop").await);
        env.restart_handler().await;
        let started = env.simple("hub.start").await;
        if NATIVE {
            applied(&started);
        } else {
            assert_eq!(error_code(&started), "unavailable");
        }
        assert_eq!(state(&env.simple("hub.status").await).1, bundle.tip);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_push_stale_authority_fails_closed() {
    timed!(async {
        for mode in 0..3u8 {
            let mut env = Env::new(0x82 + mode, options()).await.unwrap();
            let (initial, _) = pushed_and_installed(&mut env).await;
            let barrier = InitialBoundaryBarrier::install(&mut env).await;
            let bundle = env.bundle(2, Some(&initial.tip), &[("revision", b"refused")]);
            let (id, push) = begin_push(&mut env, &bundle);
            barrier.wait().await;
            assert_eq!(
                result_of(&receipt_of(&mut env, &id).await)["state"],
                "queued"
            );
            let authority = Authority::open(env.root.join("authority.sqlite")).unwrap();
            match mode {
                0 => env.clock.store(DELEGATION_EXPIRES, Ordering::SeqCst),
                1 => authority
                    .revoke_delegation(env.hub.id(), &env.coglet)
                    .unwrap(),
                _ => {
                    let renewed = Env::delegation_for(
                        &env.coglet_key,
                        &env.coglet,
                        env.hub.id().to_string(),
                        env.endpoint.id().to_string(),
                        DELEGATION_GEN + 1,
                        T0 + 100_000,
                        &env.delegation_methods,
                    );
                    authority
                        .replace_delegation(
                            env.hub.id(),
                            env.endpoint.id(),
                            &env.coglet,
                            &renewed,
                            || Some(T0),
                        )
                        .unwrap();
                }
            }
            barrier.release();
            assert_eq!(push.await.unwrap()["result"]["state"], "failed");
            // Local authorized audit oracle: revocation also correctly denies
            // remote hub.receipt, so never bypass its disclosure checks.
            let db = rusqlite::Connection::open(env.root.join("authority.sqlite")).unwrap();
            let raw: String = db
                .query_row(
                    "SELECT receipt FROM proto_git_operations WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            let receipt: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(receipt["state"], "failed", "mode {mode}: {receipt}");
            assert_eq!(receipt["operation_id"], id);
            assert_eq!(receipt["error"]["code"], "permission_denied");
            let service = env
                .handler
                .as_ref()
                .unwrap()
                .share_service(&env.coglet)
                .unwrap();
            assert_eq!(
                service.lock().unwrap().managed_status()["live_commit"],
                initial.tip
            );
            assert!(
                db.execute(
                    "UPDATE proto_git_operations SET receipt=receipt WHERE operation_id=?1",
                    [&id]
                )
                .is_err(),
                "terminal completion immutable"
            );
        }
    });
}
