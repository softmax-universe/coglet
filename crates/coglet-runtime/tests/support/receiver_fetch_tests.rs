//! Actual encrypted receiver transfers, not sender-side refusals.
use super::*;
use coglet_proto::{
    git_metadata::VerifiedRef,
    quarantine::BundleSink,
    transport::{FetchHandler, FetchResult, ReceivedTransfer, RequestContext, TransportError},
    validator::{Limits, Validator},
};
use std::{future::Future, pin::Pin};

type FetchWork<'a, T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, TransportError>> + Send + 'a>>;
struct VerifyFetch {
    metadata: Value,
    hash: String,
    authorized: AtomicU64,
    accepted: AtomicU64,
    revoke: Option<(PathBuf, Scope)>,
}
impl FetchHandler for VerifyFetch {
    fn authorize<'a>(
        &'a self,
        _: RequestContext,
        _: &'a Value,
        metadata: &'a VerifiedRef,
    ) -> FetchWork<'a, ()> {
        Box::pin(async move {
            assert_eq!(metadata.signed(), &self.metadata);
            self.authorized.fetch_add(1, Ordering::SeqCst);
            if let Some((path, scope)) = &self.revoke {
                Authority::open(path).unwrap().end_ticket(scope).unwrap();
            }
            Ok(())
        })
    }
    fn accept<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
        metadata: VerifiedRef,
        transfer: ReceivedTransfer,
        sink: BundleSink,
    ) -> FetchWork<'a, Value> {
        Box::pin(async move {
            assert_eq!(transfer.sha256(), self.hash);
            let envelope = sink
                .check_envelope(&metadata, &self.hash, Limits::default().objects)
                .await
                .map_err(|e| TransportError(e.to_string()))?;
            let bundle = envelope
                .validate(&Validator::new(validator_path()), Limits::default())
                .await
                .map_err(|e| TransportError(e.to_string()))?;
            assert_eq!(
                bundle.contents().file("logs/approved.txt"),
                Some(b"approved log\n".as_slice())
            );
            self.accepted.fetch_add(1, Ordering::SeqCst);
            Ok(json!({}))
        })
    }
}

async fn published_env(seed: u8) -> (Env, VerifyFetch) {
    let mut env = receiver_env(seed).await;
    let work = env.root.join("logs-repo");
    std::fs::create_dir_all(work.join("logs")).unwrap();
    git(&work, &["init", "--initial-branch=logs"]);
    std::fs::write(work.join("logs/approved.txt"), b"approved log\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "approved logs"]);
    let commit = String::from_utf8(git(&work, &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    let archive = git(&work, &["archive", "--format=tar", &commit]);
    git(
        &work,
        &["bundle", "create", "snapshot.bundle", "refs/heads/logs"],
    );
    let bytes = std::fs::read(work.join("snapshot.bundle")).unwrap();
    let update = sign(
        &env.coglet_key,
        "logs-ref",
        json!({"purpose":"logs-ref","hub":env.hub.id().to_string(),"coglet":env.coglet,
        "ref":"refs/heads/logs","old":null,"new":commit,"archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":1}),
    );
    let hash = hex::encode(Sha256::digest(&bytes));
    env.config
        .targets
        .get_mut(&env.coglet)
        .unwrap()
        .fetch_snapshots
        .insert(
            "refs/heads/logs".into(),
            serde_json::from_value(
                json!({"update":update,"bundle_hex":hex::encode(bytes),"sha256":hash}),
            )
            .unwrap(),
        );
    env.restart_handler().await;
    (
        env,
        VerifyFetch {
            metadata: update,
            hash,
            authorized: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            revoke: None,
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_fetch_serves_only_explicit_signed_snapshot_under_exact_scope() {
    let (mut env, observer) = published_env(210).await;
    let root = env.root.join("fetched");
    std::fs::create_dir(&root).unwrap();
    for (actor, ticket) in [
        (env.other_actor.clone(), Some(env.ticket.clone())),
        (env.actor.clone(), None),
        (env.actor.clone(), Some(NARROW_TICKET.into())),
    ] {
        let request = env.request_with(
            "hub.git.fetch",
            correlation(1000),
            json!({"ref":"refs/heads/logs"}),
            &actor,
            ticket,
        );
        let result = env
            .session
            .fetch_git(request, &root, &observer)
            .await
            .unwrap();
        let FetchResult::Rejected(result) = result else {
            panic!("unauthorized disclosure")
        };
        assert_eq!(result["error"]["code"], "permission_denied");
    }
    assert_eq!(
        observer.authorized.load(Ordering::SeqCst),
        0,
        "no metadata before scope authorization"
    );
    let request = env.request("hub.git.fetch", json!({"ref":"refs/heads/logs"}));
    assert!(matches!(
        env.session
            .fetch_git(request, &root, &observer)
            .await
            .unwrap(),
        FetchResult::Accepted(_)
    ));
    assert_eq!(observer.accepted.load(Ordering::SeqCst), 1);
    Authority::open(env.root.join("authority.sqlite"))
        .unwrap()
        .end_ticket(&Scope {
            actor: env.actor.clone(),
            ticket: Some(env.ticket.clone()),
        })
        .unwrap();
    let request = env.request("hub.git.fetch", json!({"ref":"refs/heads/logs"}));
    assert!(matches!(
        env.session
            .fetch_git(request, &root, &observer)
            .await
            .unwrap(),
        FetchResult::Rejected(_)
    ));
    assert_eq!(observer.authorized.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_fetch_rechecks_revocation_after_metadata_before_data() {
    let (mut env, mut observer) = published_env(211).await;
    let root = env.root.join("fetched");
    std::fs::create_dir(&root).unwrap();
    observer.revoke = Some((
        env.root.join("authority.sqlite"),
        Scope {
            actor: env.actor.clone(),
            ticket: Some(env.ticket.clone()),
        },
    ));
    let request = env.request("hub.git.fetch", json!({"ref":"refs/heads/logs"}));
    assert!(
        env.session
            .fetch_git(request, &root, &observer)
            .await
            .is_err()
    );
    assert_eq!(observer.authorized.load(Ordering::SeqCst), 1);
    assert_eq!(observer.accepted.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_action_expired_is_retained_without_dispatch_or_outcome() {
    let mut env = receiver_env(212).await;
    let action = json!({"id":"12".repeat(16),"coglet":env.coglet,"capability":"test.move","args":{},"deadline":0,"correlation_id":"13".repeat(16)});
    let request = env.request("hub.actions", json!({"action":action}));
    let result = env.call(request.clone()).await;
    assert_eq!(result["result"]["accepted"], false, "{result}");
    assert_eq!(result["result"]["error"]["code"], "deadline_exceeded");
    env.restart_handler().await;
    assert_eq!(env.call(request.clone()).await, result);
    let mut changed = request;
    changed["params"]["action"]["args"] = json!({"different":true});
    assert_eq!(env.call(changed).await["error"]["code"], "conflict");
    let service = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    {
        let locked = service.lock().unwrap();
        assert_eq!(locked.queued(), 0);
        assert!(locked.records().is_empty());
    }
    drop(service);
    env.serve.abort();
    let _ = (&mut env.serve).await;
    drop(env.handler.take());
    let wal = env
        .config
        .wal_root
        .join(coglet_runtime::digest(env.coglet.as_bytes()))
        .join("state.wal");
    let store = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match coglet_runtime::DurableStore::open(&wal) {
                Ok(store) => break store,
                Err(e) if e.starts_with("WAL already owned:") => tokio::task::yield_now().await,
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .unwrap();
    let state = store.state().unwrap();
    assert!(
        state["dispatches"].as_object().unwrap().is_empty(),
        "receiver rejection never prepared adapter bytes"
    );
    assert!(state["dispatch_order"].as_array().unwrap().is_empty());
    assert_eq!(
        state["receiver"]["rejected_actions"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert!(store.records().is_empty());
}
