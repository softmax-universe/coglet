//! Actual code admission only: no policy qualification or live activation.
use super::*;

struct NeverRead;
impl tokio::io::AsyncRead for NeverRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        panic!("rejected signed replay must not poll its source")
    }
}

fn code_fixture(wasm: &[u8], include_record: bool) -> (Vec<u8>, VerifiedRef) {
    let source = Root::new();
    source.git(&["init", "--initial-branch=hub"]);
    fs::create_dir(source.0.join("bin")).unwrap();
    fs::write(source.0.join("bin/step.wasm"), wasm).unwrap();
    if include_record {
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
    }
    source.git(&["add", "."]);
    source.git(&["commit", "-m", "code"]);
    let tip = String::from_utf8(source.git(&["rev-parse", "HEAD"])).unwrap();
    let archive = source.git(&["archive", "--format=tar", "HEAD"]);
    source.git(&["bundle", "create", "transfer.bundle", "refs/heads/hub"]);
    let key = SecretKey::from([1; 32]);
    let hub = key.public().to_string();
    let coglet = format!("coglet:{}", SecretKey::from([3; 32]).public());
    let statement = json!({"purpose":"hub-ref","hub":hub,"coglet":coglet,"ref":"refs/heads/hub","old":null,"new":tip.trim(),"archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":1});
    let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("hub-ref", &statement).unwrap()).to_bytes()),"signer":hub,"statement":statement});
    (
        fs::read(source.0.join("transfer.bundle")).unwrap(),
        VerifiedRef::verify(&signed, RefKind::Hub, &hub, &coglet).unwrap(),
    )
}
fn push_context(h: &Harness) -> Acceptance<'_> {
    Acceptance {
        hub: h.hub,
        coglet: h.coglet(),
        kind: RefKind::Hub,
        mode: Mode::Authoritative,
        peer: h.hub,
        scope: &h.scope,
        grant_generation: 7,
        delegation_generation: None,
    }
}
struct PushReceiver {
    h: Arc<Harness>,
    root: PathBuf,
    pause: Option<Arc<Gate>>,
    validated: AtomicUsize,
}
impl OperationHandler for PushReceiver {
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
        c: RequestContext,
        request: Value,
        mut stream: GitStream<'a>,
    ) -> Work<'a, Value> {
        Box::pin(async move {
            assert_eq!(c.peer, self.h.hub);
            assert_eq!(request["context"]["actor"], self.h.scope.actor);
            assert_eq!(
                request["context"]["ticket"],
                self.h.scope.ticket.as_deref().unwrap()
            );
            let metadata = VerifiedRef::verify(
                &request["params"]["update"],
                RefKind::Hub,
                &c.peer.to_string(),
                &self.h.coglet(),
            )
            .map_err(error)?;
            let h = self.h.clone();
            let pre = metadata.clone();
            tokio::task::spawn_blocking(move || {
                h.store
                    .precheck(&pre, &push_context(&h), || clock(c.deadline.into()))
                    .map_err(error)
            })
            .await
            .map_err(error)??;
            let sink = stream.receive_push(&metadata, &self.root).await?;
            let envelope = sink
                .check_envelope(
                    &metadata,
                    request["params"]["sha256"].as_str().unwrap(),
                    100000,
                )
                .await
                .map_err(error)?;
            let bundle = envelope
                .validate(
                    &Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator")),
                    Limits::default(),
                )
                .await
                .map_err(error)?;
            self.validated.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.pause {
                gate.entered.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
            }
            let h = self.h.clone();
            tokio::task::spawn_blocking(move || {
                h.store
                    .accept(bundle, &push_context(&h), || clock(c.deadline.into()))
                    .map_err(error)
            })
            .await
            .map_err(error)??;
            Ok(
                json!({"type":"reply","correlation_id":request["correlation_id"],"result":{"operation_id":request["correlation_id"],"state":"queued","step":null,"error":null}}),
            )
        })
    }
}

#[tokio::test]
async fn real_iroh_validated_code_push_provenance_replay_and_post_fin_slots() {
    let _fixture = SESSION_FIXTURE_LOCK.lock().await;
    tokio::time::timeout(Duration::from_secs(30),async {
        let root=Root::new(); let quarantine=Root::new();
        let h=Arc::new(live_harness(&root.0));
        let grant=Grant{method:"hub.git.push".into(),target:Some(h.coglet()),expires:clock(Instant::now()+Duration::from_secs(5)).unwrap()+3600,generation:7};
        let invite=h.authority.issue_invite(h.hub,Duration::from_secs(30),vec![grant.clone()]).unwrap();
        h.authority.enroll(h.hub,Some(&invite)).unwrap();
        h.authority.provision_scope(h.hub,&h.scope,&grant).unwrap();
        let hub=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([1;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([2;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let endpoint=hub.clone(); let incoming=tokio::spawn(async move {endpoint.accept().await.unwrap().await.unwrap()});
        let dc=device.connect(hub.addr(),ALPN).await.unwrap(); let hc=incoming.await.unwrap();
        let ds=Session::new(dc,device.id(),Role::Device).unwrap(); let hs=Session::new(hc.clone(),hub.id(),Role::Hub).unwrap();
        let gate=Arc::new(Gate{entered:tokio::sync::Semaphore::new(0),release:tokio::sync::Semaphore::new(0)});
        let receiver=Arc::new(PushReceiver{h:h.clone(),root:quarantine.0.clone(),pause:Some(gate.clone()),validated:AtomicUsize::new(0)});
        let hub_job=tokio::spawn(hs.clone().serve(receiver.clone()));
        ds.request(json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":5000,"params":{"offer":{"versions":["0.1"]},"invite":null}})).await.unwrap();
        let device_job=tokio::spawn(ds.serve(receiver.clone()));
        let (bytes,metadata)=code_fixture(b"\0asm\x01\0\0\0",true);
        let make_request=|id:&str,bytes:&[u8],m:&VerifiedRef| json!({"method":"hub.git.push","target":h.coglet(),"context":{"actor":h.scope.actor,"ticket":h.scope.ticket},"correlation_id":id.repeat(16),"timeout_ms":5000,"params":{"update":m.signed(),"size":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))}});
        // Valid signatures and transport hashes cannot authorize invalid code.
        for (id,module,record) in [("a1",b"invalid".as_slice(),true),("a2",b"\0asm\x01\0\0\0".as_slice(),false)] {
            let (bad,m)=code_fixture(module,record);
            assert!(hs.push_git(make_request(id,&bad,&m),&mut bad.as_slice()).await.is_err());
            assert_eq!(h.count("proto_ref_audit"),0);
            assert_quarantines(&quarantine.0,0).await;
        }
        // Both responder validations hold their slots after complete end+FIN.
        let request=make_request("cd",&bytes,&metadata);
        let session=hs.clone(); let r=request.clone(); let b=bytes.clone();
        let one=tokio::spawn(async move {session.push_git(r,&mut b.as_slice()).await});
        gate.entered.acquire().await.unwrap().forget();
        let session=hs.clone(); let r=make_request("ce",&bytes,&metadata); let b=bytes.clone();
        let two=tokio::spawn(async move {session.push_git(r,&mut b.as_slice()).await});
        gate.entered.acquire().await.unwrap().forget();
        assert_eq!(receiver.validated.load(Ordering::SeqCst),2);
        assert_eq!(fs::read_dir(&quarantine.0).unwrap().count(),2);
        assert!(hs.push_git(make_request("cf",&bytes,&metadata),&mut bytes.as_slice()).await.is_err());
        // Bypass caller admission: the responder must reject independently.
        let (mut raw_send,mut raw_recv)=hc.open_bi().await.unwrap();
        coglet_proto::wire::write_frame(&mut raw_send,&make_request("c1",&bytes,&metadata)).await.unwrap();
        let refusal=coglet_proto::wire::read_frame(&mut raw_recv).await.unwrap();
        assert_eq!(refusal["error"]["code"],"resource_exhausted");
        assert!(raw_recv.read(&mut [0]).await.unwrap().is_none());
        assert_eq!(receiver.validated.load(Ordering::SeqCst),2);
        assert_eq!(h.count("proto_ref_audit"),0);
        gate.release.add_permits(2);
        let outcomes=[one.await.unwrap(),two.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|v|matches!(v,Ok(reply) if reply["result"]["state"]=="queued")).count(),1);
        assert_eq!(h.count("proto_ref_audit"),1,"concurrent authoritative CAS admits exactly one");
        assert_quarantines(&quarantine.0,0).await;
        let current=h.store.current(&h.hub.to_string(),&h.coglet(),RefKind::Hub).unwrap().unwrap();
        assert_eq!(current.metadata.signed(),metadata.signed());
        assert_eq!(h.store.audit_bundle(current.audit_id).unwrap(),bytes);
        assert!(hs.push_git(make_request("d1",&bytes,&metadata),&mut NeverRead).await.is_err(),"new operation signed replay denies before credit/source read");
        assert_eq!(h.count("proto_ref_audit"),1);
        hub_job.abort(); device_job.abort(); let _=hub_job.await; let _=device_job.await;
        hub.close().await; device.close().await;
    }).await.unwrap();
}
