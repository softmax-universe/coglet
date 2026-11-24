//! Actual Git/validator/store integration over the public Session API. Not activation.
use super::*;
use coglet_proto::transport::{
    ALPN, FetchHandler, FetchResult, GitStream, OperationHandler, RequestContext, Role, Session,
    TransportError,
};
use iroh::{Endpoint, endpoint::presets};
use serde_json::Value;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

type Work<'a, T> = Pin<Box<dyn Future<Output = Result<T, TransportError>> + Send + 'a>>;
fn error(e: impl std::fmt::Display) -> TransportError {
    TransportError(e.to_string())
}
fn clock(deadline: Instant) -> Option<u64> {
    (Instant::now() < deadline)
        .then(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        })
        .flatten()
}
fn live_harness(root: &Path) -> Harness {
    let h = Harness::new(root);
    let expires = clock(Instant::now() + Duration::from_secs(5)).unwrap() + 3600;
    // Trusted fixture provisioning, not a remote authority override.
    Connection::open(&h.path).unwrap().execute_batch(&format!("UPDATE proto_grants SET expires='{expires}'; UPDATE proto_scopes SET expires='{expires}';")).unwrap();
    let key = SecretKey::from([3; 32]);
    let statement = json!({"purpose":"delegation","coglet":h.coglet(),"hub":h.hub.to_string(),"device":h.peer.to_string(),"generation":10,"expires":expires,"methods":["hub.git.push","hub.git.fetch"]});
    let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("delegation", &statement).unwrap()).to_bytes()),"signer":key.public().to_string(),"statement":statement});
    h.authority
        .replace_delegation(h.hub, h.peer, &h.coglet(), &signed, || {
            clock(Instant::now() + Duration::from_secs(5))
        })
        .unwrap();
    h
}
struct Source {
    snapshot: Mutex<(VerifiedRef, Vec<u8>)>,
    completed: AtomicUsize,
}
impl OperationHandler for Source {
    fn handle<'a>(
        &'a self,
        c: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            json!({"type":"reply","correlation_id":request["correlation_id"],"result":{"selection":{"version":"0.1"},"hub":c.local.to_string(),"device":c.peer.to_string()}})
        })
    }
    fn handle_git<'a>(
        &'a self,
        _: RequestContext,
        request: Value,
        mut stream: GitStream<'a>,
    ) -> Work<'a, Value> {
        Box::pin(async move {
            let (metadata, bytes) = self.snapshot.lock().unwrap().clone();
            stream
                .send_fetch(
                    &metadata,
                    bytes.len() as u64,
                    &hex::encode(Sha256::digest(&bytes)),
                    &mut bytes.as_slice(),
                )
                .await?;
            self.completed.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"type":"reply","correlation_id":request["correlation_id"],"result":{}}))
        })
    }
}
struct Gate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}
struct Consumer {
    pause: Option<Arc<Gate>>,
    h: Arc<Harness>,
    limits: Limits,
    hash: String,
    validated: AtomicUsize,
}
impl FetchHandler for Consumer {
    fn authorize<'a>(
        &'a self,
        c: RequestContext,
        _: &'a Value,
        metadata: &'a VerifiedRef,
    ) -> Work<'a, ()> {
        Box::pin(async move {
            assert_eq!(c.local, self.h.hub);
            assert_eq!(c.peer, self.h.peer);
            let h = self.h.clone();
            let metadata = metadata.clone();
            tokio::task::spawn_blocking(move || {
                let mut context = h.context(Mode::Observation);
                context.delegation_generation = Some(10);
                h.store
                    .precheck(&metadata, &context, || clock(c.deadline.into()))
                    .map_err(error)
            })
            .await
            .map_err(error)?
        })
    }
    fn accept<'a>(
        &'a self,
        c: RequestContext,
        _: Value,
        metadata: VerifiedRef,
        sink: BundleSink,
    ) -> Work<'a, Value> {
        Box::pin(async move {
            let envelope = sink
                .check_envelope(&metadata, &self.hash, 100000)
                .await
                .map_err(error)?;
            let bundle = envelope
                .validate(
                    &Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator")),
                    self.limits,
                )
                .await
                .map_err(error)?;
            self.validated.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.pause {
                gate.entered.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
            }
            let h = self.h.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                let mut context = h.context(Mode::Observation);
                context.delegation_generation = Some(10);
                h.store
                    .accept(bundle, &context, || clock(c.deadline.into()))
                    .map_err(error)
            })
            .await
            .map_err(error)??;
            Ok(json!({"audit_id":receipt.audit_id,"changed":receipt.changed}))
        })
    }
}

#[tokio::test]
async fn real_iroh_validated_git_fetch_reopen_statement_revalidation_and_limits() {
    tokio::time::timeout(Duration::from_secs(30),async {
        let root=Root::new(); let quarantine=Root::new();
        let bytes=fixture(); let first=metadata(&bytes,1,None);
        let h=Arc::new(live_harness(&root.0));
        let hub=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([1;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([2;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let endpoint=hub.clone(); let incoming=tokio::spawn(async move { endpoint.accept().await.unwrap().await.unwrap() });
        let dc=device.connect(hub.addr(),ALPN).await.unwrap(); let hc=incoming.await.unwrap();
        let ds=Session::new(dc,device.id(),Role::Device).unwrap(); let hs=Session::new(hc,hub.id(),Role::Hub).unwrap();
        let source=Arc::new(Source{snapshot:Mutex::new((first.clone(),bytes.clone())),completed:AtomicUsize::new(0)});
        let hub_job=tokio::spawn(hs.clone().serve(source.clone()));
        ds.request(json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":5000,"params":{"offer":{"versions":["0.1"]},"invite":null}})).await.unwrap();
        let device_job=tokio::spawn(ds.serve(source.clone()));
        let request=json!({"method":"hub.git.fetch","target":h.coglet(),"context":{"actor":h.scope.actor,"ticket":h.scope.ticket},"correlation_id":"cd".repeat(16),"timeout_ms":5000,"params":{"ref":"refs/heads/logs"}});
        let consumer=Consumer{pause:None,hash:hex::encode(Sha256::digest(&bytes)),h:h.clone(),limits:Limits::default(),validated:AtomicUsize::new(0)};
        let FetchResult::Accepted(receipt)=hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.unwrap() else {panic!("rejected")};
        assert_eq!(receipt["changed"],true); assert_eq!(h.current().unwrap().metadata.signed(),first.signed());
        assert_eq!(h.store.audit_bundle(receipt["audit_id"].as_i64().unwrap()).unwrap(),bytes);
        let FetchResult::Accepted(retry)=hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.unwrap() else {panic!("rejected")};
        assert_eq!(retry["audit_id"],receipt["audit_id"]); assert_eq!(retry["changed"],false);
        assert_eq!(consumer.validated.load(Ordering::SeqCst),2,"identical statements still validate real Git");
        let reopened=Arc::new(Harness::open(&root.0));
        assert_eq!(reopened.current().unwrap().metadata.signed(),first.signed());
        let consumer=Consumer{pause:None,hash:hex::encode(Sha256::digest(&bytes)),h:reopened,limits:Limits{expanded_bytes:1,..Limits::default()},validated:AtomicUsize::new(0)};
        assert!(hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.unwrap_err().0.contains("resource_exhausted"));
        assert_eq!(h.count("proto_ref_audit"),1); assert_eq!(fs::read_dir(&quarantine.0).unwrap().count(),0);
        // Even an identical signed statement cannot launder a corrupted pack.
        let mut corrupt=bytes.clone(); *corrupt.last_mut().unwrap() ^= 1;
        *source.snapshot.lock().unwrap()=(first.clone(),corrupt.clone());
        let consumer=Consumer{pause:None,hash:hex::encode(Sha256::digest(&corrupt)),h:h.clone(),limits:Limits::default(),validated:AtomicUsize::new(0)};
        assert!(hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.is_err());
        assert_eq!(consumer.validated.load(Ordering::SeqCst),0);
        assert_eq!(h.count("proto_ref_audit"),1); assert_eq!(fs::read_dir(&quarantine.0).unwrap().count(),0);
        // Skipped publisher generations/old tips are legal for observations.
        let newer_bytes=fixture_data(b"{\"event\":3}\n");
        let newer=metadata(&newer_bytes,3,Some(&"ff".repeat(20)));
        assert_ne!(newer.new_tip(),first.new_tip());
        *source.snapshot.lock().unwrap()=(newer.clone(),newer_bytes.clone());
        let consumer=Consumer{pause:None,hash:hex::encode(Sha256::digest(&newer_bytes)),h:h.clone(),limits:Limits::default(),validated:AtomicUsize::new(0)};
        assert!(matches!(hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.unwrap(),FetchResult::Accepted(v) if v["changed"]==true));
        assert_eq!(h.current().unwrap().metadata.generation(),3);
        // Observe actual no-credit rejection: responder never finishes send,
        // nonexistent quarantine root remains absent, and audit never advances.
        let completed=source.completed.load(Ordering::SeqCst);
        for stale in [first.clone(),metadata(&newer_bytes,3,None)] {
            *source.snapshot.lock().unwrap()=(stale,bytes.clone());
            assert!(hs.fetch_git(request.clone(),&quarantine.0.join("absent"),&consumer).await.unwrap_err().0.contains("conflict"));
            assert!(!quarantine.0.join("absent").exists());
        }
        assert_eq!(source.completed.load(Ordering::SeqCst),completed);
        assert_eq!(h.count("proto_ref_audit"),2);
        *source.snapshot.lock().unwrap()=(newer,newer_bytes.clone());
        let gate=Arc::new(Gate{entered:tokio::sync::Semaphore::new(0),release:tokio::sync::Semaphore::new(0)});
        let paused=Consumer{pause:Some(gate.clone()),hash:hex::encode(Sha256::digest(&newer_bytes)),h:h.clone(),limits:Limits::default(),validated:AtomicUsize::new(0)};
        let session=hs.clone(); let pending_request=request.clone(); let pending_root=quarantine.0.clone();
        let pending=tokio::spawn(async move {session.fetch_git(pending_request,&pending_root,&paused).await});
        gate.entered.acquire().await.unwrap().forget();
        assert_eq!(fs::read_dir(&quarantine.0).unwrap().count(),1,"validated custody survives terminal FIN until commit");
        h.authority.revoke_delegation(h.hub,&h.coglet()).unwrap();
        gate.release.add_permits(1);
        assert!(pending.await.unwrap().unwrap_err().0.contains("permission_denied"),"commit must recheck authority after validation");
        assert_eq!(fs::read_dir(&quarantine.0).unwrap().count(),0);
        assert!(hs.fetch_git(request.clone(),&quarantine.0.join("absent"),&consumer).await.unwrap_err().0.contains("permission_denied"));
        assert!(!quarantine.0.join("absent").exists());
        assert_eq!(h.count("proto_ref_audit"),2);
        hub_job.abort(); device_job.abort(); let _=hub_job.await; let _=device_job.await;
        hub.close().await; device.close().await;
    }).await.unwrap();
}
