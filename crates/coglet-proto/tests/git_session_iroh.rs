//! Streaming Session transport tests, not code activation or full Git acceptance.
use coglet_proto::{
    git_metadata::{RefKind, VerifiedRef},
    signing,
    transport::{ALPN, GitStream, OperationHandler, RequestContext, Role, Session},
    wire::{read_frame, write_frame},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

struct RawFetch;
impl coglet_proto::transport::FetchHandler for RawFetch {
    fn authorize<'a>(
        &'a self,
        _: RequestContext,
        _: &'a Value,
        metadata: &'a VerifiedRef,
    ) -> Pin<
        Box<dyn Future<Output = Result<(), coglet_proto::transport::TransportError>> + Send + 'a>,
    > {
        Box::pin(async move {
            assert_eq!(metadata.signed()["statement"]["generation"], 1);
            Ok(())
        })
    }
    fn accept<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
        _: VerifiedRef,
        _: coglet_proto::transport::ReceivedTransfer,
        sink: coglet_proto::quarantine::BundleSink,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Value, coglet_proto::transport::TransportError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            drop(sink);
            Ok(json!("raw-only"))
        })
    }
}

struct DenyFetch;
impl coglet_proto::transport::FetchHandler for DenyFetch {
    fn authorize<'a>(
        &'a self,
        _: RequestContext,
        _: &'a Value,
        _: &'a VerifiedRef,
    ) -> Pin<
        Box<dyn Future<Output = Result<(), coglet_proto::transport::TransportError>> + Send + 'a>,
    > {
        Box::pin(async {
            Err(coglet_proto::transport::TransportError(
                "local conflict".into(),
            ))
        })
    }
    fn accept<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
        _: VerifiedRef,
        _: coglet_proto::transport::ReceivedTransfer,
        _: coglet_proto::quarantine::BundleSink,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Value, coglet_proto::transport::TransportError>> + Send + 'a,
        >,
    > {
        panic!("denied metadata cannot reach acceptance")
    }
}

struct HoldFetch {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}
impl coglet_proto::transport::FetchHandler for HoldFetch {
    fn authorize<'a>(
        &'a self,
        _: RequestContext,
        _: &'a Value,
        _: &'a VerifiedRef,
    ) -> Pin<
        Box<dyn Future<Output = Result<(), coglet_proto::transport::TransportError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }
    fn accept<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
        _: VerifiedRef,
        _: coglet_proto::transport::ReceivedTransfer,
        sink: coglet_proto::quarantine::BundleSink,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Value, coglet_proto::transport::TransportError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            self.entered.add_permits(1);
            let _sink = sink;
            self.release.acquire().await.unwrap().forget();
            Ok(json!("released"))
        })
    }
}

struct Service {
    root: PathBuf,
}
impl OperationHandler for Service {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(request["method"], "hub.enroll");
            json!({"type":"reply","correlation_id":request["correlation_id"],"result":{
                "selection":{"version":"0.1"},"hub":context.local.to_string(),"device":context.peer.to_string()}})
        })
    }
    fn handle_git<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
        mut stream: GitStream<'a>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Value, coglet_proto::transport::TransportError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            if request["method"] == "hub.git.fetch" {
                let key = SecretKey::from([3; 32]);
                let statement = json!({"purpose":"logs-ref","hub":context.peer.to_string(),"coglet":request["target"],
                    "ref":"refs/heads/logs","old":null,"new":"12".repeat(20),"archive_sha256":"34".repeat(32),"generation":1});
                let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("logs-ref", &statement).unwrap()).to_bytes()),
                    "signer":key.public().to_string(),"statement":statement});
                let metadata = VerifiedRef::verify(
                    &signed,
                    RefKind::Logs,
                    &context.peer.to_string(),
                    request["target"].as_str().unwrap(),
                )
                .unwrap();
                let bytes = vec![8; 150_000];
                stream
                    .send_fetch(
                        &metadata,
                        bytes.len() as u64,
                        &hex::encode(Sha256::digest(&bytes)),
                        &mut bytes.as_slice(),
                    )
                    .await?;
                return Ok(
                    json!({"type":"reply","correlation_id":request["correlation_id"],"result":{}}),
                );
            }
            let metadata = VerifiedRef::verify(
                &request["params"]["update"],
                RefKind::Hub,
                &context.peer.to_string(),
                request["target"].as_str().unwrap(),
            )
            .unwrap();
            // This fixture deliberately tests raw data only. No ref is accepted.
            let sink = stream.receive_push(&metadata, &self.root).await?;
            drop(sink);
            Ok(
                json!({"type":"reply","correlation_id":request["correlation_id"],"result":{
                "operation_id":request["correlation_id"],"state":"queued","step":null,"error":null}}),
            )
        })
    }
}

#[tokio::test]
async fn real_iroh_session_dispatches_push_before_fin_and_fin_precedes_handler_success() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let root = std::env::temp_dir().join(format!("coglet-session-{}", SecretKey::generate().public()));
        std::fs::create_dir(&root).unwrap();
        let key = SecretKey::generate();
        let hub = Endpoint::builder(presets::Minimal).secret_key(key.clone()).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let h = hub.clone();
        let incoming = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
        let dc = device.connect(hub.addr(), ALPN).await.unwrap();
        let hc = incoming.await.unwrap();
        let ds = Session::new(dc, device.id(), Role::Device).unwrap();
        let hs = Session::new(hc.clone(), hub.id(), Role::Hub).unwrap();
        let service = Arc::new(Service { root:root.clone() });
        let initiator = hs.clone();
        let serving_hub = tokio::spawn(hs.serve(service.clone()));
        let enrollment = json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":2000,
            "params":{"offer":{"versions":["0.1"]},"invite":null}});
        assert_eq!(ds.request(enrollment).await.unwrap()["type"], "reply");
        let serving_device = tokio::spawn(ds.serve(service));
        let target = format!("coglet:{}", SecretKey::generate().public());
        let statement = json!({"purpose":"hub-ref","hub":hub.id().to_string(),"coglet":target,"ref":"refs/heads/hub", "old":null,
            "new":"12".repeat(20),"archive_sha256":"34".repeat(32),"generation":1});
        let update = json!({"signature":hex::encode(key.sign(&signing::preimage("hub-ref", &statement).unwrap()).to_bytes()),
            "signer":hub.id().to_string(),"statement":statement});
        let bytes = vec![7;150_000];
        let id = "cd".repeat(16);
        let request = json!({"method":"hub.git.push","target":target,"context":{"actor":format!("hub:{}",hub.id())},"correlation_id":id,"timeout_ms":5000,
            "params":{"update":update,"size":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}});
        assert_eq!(initiator.push_git(request.clone(), &mut bytes.as_slice()).await.unwrap()["result"]["state"], "queued");
        let (mut send, mut recv) = hc.open_bi().await.unwrap();
        write_frame(&mut send, &request).await.unwrap();
        coglet_proto::transfer::send_chunks(&mut send, &mut recv, &id, bytes.len() as u64, &mut bytes.as_slice()).await.unwrap();
        // No success before FIN: the receive helper must fence trailing bytes.
        assert!(tokio::time::timeout(Duration::from_millis(50), read_frame(&mut recv)).await.is_err());
        send.finish().unwrap();
        assert_eq!(read_frame(&mut recv).await.unwrap()["result"]["state"], "queued");
        assert!(recv.read(&mut [0]).await.unwrap().is_none());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),0);
        // Fetch keeps the request direction open for credit until end, not FIN
        // immediately after the request (that would deadlock multi-window flow).
        let fetch_id = "ef".repeat(16);
        let fetch = json!({"method":"hub.git.fetch","target":format!("coglet:{}",SecretKey::from([3;32]).public()),
            "context":{"actor":format!("hub:{}",hub.id())},"correlation_id":fetch_id,"timeout_ms":5000,"params":{"ref":"refs/heads/logs"}});
        assert_eq!(initiator.request(fetch.clone()).await.unwrap_err().0, "Git requires streaming API");
        assert!(matches!(initiator.fetch_git(fetch.clone(), &root, &RawFetch).await.unwrap(),
            coglet_proto::transport::FetchResult::Accepted(value) if value == "raw-only"));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),0);
        assert!(initiator.fetch_git(fetch.clone(), &root.join("absent"), &DenyFetch).await.unwrap_err().0.contains("local conflict"));
        assert!(!root.join("absent").exists(),"authorization precedes quarantine creation");
        // Both wire transfers and terminal FINs finish, but local acceptance
        // still owns the two shared Git slots and quarantine custody.
        let held = Arc::new(HoldFetch { entered:tokio::sync::Semaphore::new(0), release:tokio::sync::Semaphore::new(0) });
        let mut jobs = Vec::new();
        for index in 0..2 {
            let session = initiator.clone(); let root = root.clone(); let held = held.clone(); let mut request = fetch.clone();
            request["correlation_id"] = json!(format!("{:032x}",index + 1));
            jobs.push(tokio::spawn(async move { session.fetch_git(request,&root,held.as_ref()).await }));
        }
        held.entered.acquire_many(2).await.unwrap().forget();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),2);
        assert!(initiator.fetch_git(fetch.clone(),&root,&RawFetch).await.is_err(),"local acceptance must retain both slots after FIN");
        held.release.add_permits(2);
        for job in jobs { assert!(matches!(job.await.unwrap().unwrap(),coglet_proto::transport::FetchResult::Accepted(_))); }
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),0);
        let session = initiator.clone(); let local_root = root.clone(); let local_held = held.clone(); let mut short = fetch.clone();
        // Leave disk-backed multi-window reception a full request budget; the
        // observed acceptance gate, not transfer speed, is the timeout subject.
        short["timeout_ms"] = json!(5000);
        let deadline_job = tokio::spawn(async move { session.fetch_git(short,&local_root,local_held.as_ref()).await });
        held.entered.acquire().await.unwrap().forget();
        assert!(deadline_job.await.unwrap().is_err(),"deadline includes local acceptance");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),0);
        assert!(matches!(initiator.fetch_git(fetch.clone(), &root, &RawFetch).await.unwrap(),coglet_proto::transport::FetchResult::Accepted(_)),"deadline releases permit");
        let (mut send, mut recv) = hc.open_bi().await.unwrap();
        write_frame(&mut send,&fetch).await.unwrap();
        let transfer = read_frame(&mut recv).await.unwrap();
        assert_eq!(transfer["type"],"transfer");
        let mut sink = coglet_proto::quarantine::BundleSink::create(&root,150_000).await.unwrap();
        coglet_proto::transfer::receive_chunks(&mut send,&mut recv,&fetch_id,150_000,transfer["sha256"].as_str().unwrap(),&mut sink).await.unwrap();
        send.finish().unwrap();
        assert_eq!(read_frame(&mut recv).await.unwrap()["type"],"reply");
        assert!(recv.read(&mut [0]).await.unwrap().is_none());
        drop(sink);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),0);
        for trailing in [false,true] {
            let mut malformed = request.clone();
            malformed["timeout_ms"] = json!(500);
            malformed["params"]["size"] = json!(1);
            malformed["params"]["sha256"] = json!(hex::encode(Sha256::digest([7])));
            let (mut send,mut recv) = hc.open_bi().await.unwrap();
            write_frame(&mut send,&malformed).await.unwrap();
            coglet_proto::transfer::send_chunks(&mut send,&mut recv,&id,1,&mut [7].as_slice()).await.unwrap();
            if trailing {
                write_frame(&mut send,&json!({"type":"end","correlation_id":id})).await.unwrap();
                send.finish().unwrap();
            }
            assert!(matches!(recv.read(&mut [0]).await,
                Err(iroh::endpoint::ReadError::Reset(code)) if code.into_inner()==1));
            assert_eq!(std::fs::read_dir(&root).unwrap().count(),0,"rejection removes raw quarantine before reset");
        }
        serving_hub.abort(); serving_device.abort();
        let _ = serving_hub.await; let _ = serving_device.await;
        device.close().await; hub.close().await;
        std::fs::remove_dir(root).unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn real_iroh_legacy_handler_is_not_invoked_for_streaming_git() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Legacy(Arc<AtomicUsize>);
    impl OperationHandler for Legacy {
        fn handle<'a>(
            &'a self,
            _context: RequestContext,
            request: Value,
        ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                json!({"type":"reply","correlation_id":request["correlation_id"],"result":{}})
            })
        }
    }
    tokio::time::timeout(Duration::from_secs(10),async {
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let h = hub.clone();
        let incoming = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
        let dc = device.connect(hub.addr(),ALPN).await.unwrap();
        let hc = incoming.await.unwrap();
        let ds = Session::new(dc,device.id(),Role::Device).unwrap();
        let hs = Session::new(hc.clone(),hub.id(),Role::Hub).unwrap();
        let initiator = hs.clone();
        let serving_hub = tokio::spawn(hs.serve(Arc::new(Service {root:PathBuf::new()})));
        ds.request(json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":1000,
            "params":{"offer":{"versions":["0.1"]},"invite":null}})).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let serving_device = tokio::spawn(ds.serve(Arc::new(Legacy(calls.clone()))));
        let (mut send,mut recv) = hc.open_bi().await.unwrap();
        write_frame(&mut send,&json!({"method":"hub.git.fetch","target":format!("coglet:{}",SecretKey::from([3;32]).public()),
            "context":{"actor":format!("hub:{}",hub.id())},"correlation_id":"cd".repeat(16),"timeout_ms":1000,"params":{"ref":"refs/heads/logs"}})).await.unwrap();
        let reply = read_frame(&mut recv).await;
        assert_eq!(calls.load(Ordering::SeqCst),0,"legacy mutation dispatch must not run");
        assert_eq!(reply.unwrap()["error"]["code"],"permission_denied");
        assert_eq!(send.stopped().await.unwrap().unwrap().into_inner(),1);
        let target = format!("coglet:{}",SecretKey::from([3;32]).public());
        let push = json!({"method":"hub.git.push","target":target,
            "context":{"actor":format!("hub:{}",hub.id())},"correlation_id":"ef".repeat(16),"timeout_ms":1000,
            "params":{"size":150000,"sha256":"ab".repeat(32),"update":{"signer":hub.id().to_string(),"signature":"00".repeat(64),
                "statement":{"purpose":"hub-ref","hub":hub.id().to_string(),"coglet":target,"ref":"refs/heads/hub","old":null,
                "new":"12".repeat(20),"archive_sha256":"34".repeat(32),"generation":1}}}});
        assert_eq!(initiator.push_git(push,&mut tokio::io::empty()).await.unwrap()["error"]["code"],"permission_denied",
            "valid rejection before credit must not read even an empty source");
        assert_eq!(calls.load(Ordering::SeqCst),0);
        serving_device.abort(); serving_hub.abort();
        let _ = serving_device.await; let _ = serving_hub.await;
        device.close().await; hub.close().await;
    }).await.unwrap();
}
