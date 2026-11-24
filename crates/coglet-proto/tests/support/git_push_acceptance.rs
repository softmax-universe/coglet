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
    code_fixture_for(wasm, include_record, false)
}
fn code_fixture_for(wasm: &[u8], include_record: bool, published: bool) -> (Vec<u8>, VerifiedRef) {
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
    let (reference, purpose, kind) = if published {
        ("refs/published/main", "published-ref", RefKind::Published)
    } else {
        ("refs/heads/hub", "hub-ref", RefKind::Hub)
    };
    if published {
        source.git(&["update-ref", reference, "HEAD"]);
    }
    source.git(&["bundle", "create", "transfer.bundle", reference]);
    let key = SecretKey::from(if published { [3; 32] } else { [1; 32] });
    let hub = SecretKey::from([1; 32]).public().to_string();
    let coglet = format!("coglet:{}", SecretKey::from([3; 32]).public());
    let statement = json!({"purpose":purpose,"hub":hub,"coglet":coglet,"ref":reference,"old":null,"new":tip.trim(),"archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":1});
    let signed = json!({"signature":hex::encode(key.sign(&signing::preimage(purpose, &statement).unwrap()).to_bytes()),"signer":key.public().to_string(),"statement":statement});
    (
        fs::read(source.0.join("transfer.bundle")).unwrap(),
        VerifiedRef::verify(&signed, kind, &hub, &coglet).unwrap(),
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
            let operation =
                coglet_proto::authority::accepted_refs::PushOperation::new(c.peer, &request)
                    .map_err(error)?;
            let h = self.h.clone();
            let op = operation.clone();
            let pre = metadata.clone();
            tokio::task::spawn_blocking(move || {
                h.store
                    .precheck_push(&pre, &push_context(&h), &op, || clock(c.deadline.into()))
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
            let receipt = tokio::task::spawn_blocking(move || {
                h.store
                    .accept_push(bundle, &push_context(&h), &operation, || {
                        clock(c.deadline.into())
                    })
                    .map_err(error)
            })
            .await
            .map_err(error)??;
            Ok(json!({"type":"reply","correlation_id":request["correlation_id"],"result":receipt}))
        })
    }
}

#[tokio::test]
async fn real_iroh_validated_published_code_fetch_checks_wasm_and_provenance() {
    let _fixture = SESSION_FIXTURE_LOCK.lock().await;
    tokio::time::timeout(Duration::from_secs(20),async {
        let root=Root::new();let quarantine=Root::new();let h=Arc::new(live_harness(&root.0));
        let (bytes,metadata)=code_fixture_for(b"\0asm\x01\0\0\0",true,true);
        let source=Arc::new(Source{snapshot:Mutex::new((metadata.clone(),bytes.clone())),completed:AtomicUsize::new(0)});
        let hub=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([1;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([2;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let endpoint=hub.clone();let incoming=tokio::spawn(async move {endpoint.accept().await.unwrap().await.unwrap()});
        let dc=device.connect(hub.addr(),ALPN).await.unwrap();let hc=incoming.await.unwrap();
        let ds=Session::new(dc,device.id(),Role::Device).unwrap();let hs=Session::new(hc,hub.id(),Role::Hub).unwrap();
        let hub_job=tokio::spawn(hs.clone().serve(source.clone()));
        ds.request(json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":5000,"params":{"offer":{"versions":["0.1"]},"invite":null}})).await.unwrap();
        let device_job=tokio::spawn(ds.serve(source.clone()));
        let request=json!({"method":"hub.git.fetch","target":h.coglet(),"context":{"actor":h.scope.actor,"ticket":h.scope.ticket},"correlation_id":"fb".repeat(16),"timeout_ms":5000,"params":{"ref":"refs/published/main"}});
        let consumer=Consumer{kind:RefKind::Published,pause:None,h:h.clone(),limits:Limits::default(),validated:AtomicUsize::new(0)};
        for (wasm,record) in [(b"invalid".as_slice(),true),(b"\0asm\x01\0\0\0".as_slice(),false)] {
            let (bad, signed)=code_fixture_for(wasm,record,true);
            *source.snapshot.lock().unwrap()=(signed,bad);
            assert!(hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.is_err());
            assert_eq!(h.count("proto_ref_audit"),0);assert_quarantines(&quarantine.0,0).await;
        }
        *source.snapshot.lock().unwrap()=(metadata.clone(),bytes.clone());
        let FetchResult::Accepted(receipt)=hs.fetch_git(request.clone(),&quarantine.0,&consumer).await.unwrap() else {panic!("rejected")};
        assert_eq!(receipt["changed"],true);
        let current=h.store.current(&h.hub.to_string(),&h.coglet(),RefKind::Published).unwrap().unwrap();
        assert_eq!(current.metadata.signed(),metadata.signed());assert_eq!(h.store.audit_bundle(current.audit_id).unwrap(),bytes);
        let FetchResult::Accepted(retry)=hs.fetch_git(request,&quarantine.0,&consumer).await.unwrap() else {panic!("rejected")};
        assert_eq!(retry["changed"],false);assert_eq!(consumer.validated.load(Ordering::SeqCst),2);assert_eq!(h.count("proto_ref_audit"),1);
        assert_quarantines(&quarantine.0,0).await;
        hub_job.abort();device_job.abort();let _=hub_job.await;let _=device_job.await;hub.close().await;device.close().await;
    }).await.unwrap();
}

#[test]
fn real_iroh_code_push_process_restart_replays_original_receipt() {
    let root = Root::new();
    for phase in ["admit", "recover"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "git_acceptance_session::git_push_acceptance::session_restart_child",
                "--ignored",
                "--nocapture",
            ])
            .env("COGLET_SESSION_RESTART_ROOT", &root.0)
            .env("COGLET_SESSION_RESTART_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(if phase == "admit" { 71 } else { 72 }));
    }
    let h = Harness::open(&root.0);
    assert_eq!(h.count("proto_ref_audit"), 1);
    assert_eq!(h.count("proto_git_operations"), 1);
}

#[tokio::test]
#[ignore = "subprocess helper: requires private restart fixture"]
async fn session_restart_child() {
    let Ok(path) = std::env::var("COGLET_SESSION_RESTART_ROOT") else {
        panic!("requires restart fixture")
    };
    tokio::time::timeout(Duration::from_secs(20),async {
        let path=PathBuf::from(path);
        let recovery=std::env::var("COGLET_SESSION_RESTART_PHASE").unwrap()=="recover";
        let h=Arc::new(if recovery {Harness::open(&path)} else {live_harness(&path)});
        let quarantine=path.join("quarantine");fs::create_dir_all(&quarantine).unwrap();
        BundleSink::sweep_orphans(&quarantine).await.unwrap();
        if !recovery {
            let grant=Grant{method:"hub.git.push".into(),target:Some(h.coglet()),expires:clock(Instant::now()+Duration::from_secs(5)).unwrap()+3600,generation:7};
            let invite=h.authority.issue_invite(h.hub,Duration::from_secs(30),vec![grant.clone()]).unwrap();
            h.authority.enroll(h.hub,Some(&invite)).unwrap();h.authority.provision_scope(h.hub,&h.scope,&grant).unwrap();
        }
        let (bytes,metadata)=if recovery {
            let current=h.store.current(&h.hub.to_string(),&h.coglet(),RefKind::Hub).unwrap().unwrap();
            (h.store.audit_bundle(current.audit_id).unwrap(),current.metadata)
        } else {code_fixture(b"\0asm\x01\0\0\0",true)};
        let request=json!({"method":"hub.git.push","target":h.coglet(),"context":{"actor":h.scope.actor,"ticket":h.scope.ticket},"correlation_id":"fa".repeat(16),"timeout_ms":if recovery{10000}else{5000},"params":{"update":metadata.signed(),"size":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}});
        let op=coglet_proto::authority::accepted_refs::PushOperation::new(h.hub,&request).unwrap();
        let recovered=h.store.push_receipt(&metadata,&push_context(&h),&op,||clock(Instant::now()+Duration::from_secs(5))).unwrap();
        assert_eq!(recovered.is_some(),recovery);
        let hub=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([1;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device=Endpoint::builder(presets::Minimal).secret_key(SecretKey::from([2;32])).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let endpoint=hub.clone();let incoming=tokio::spawn(async move {endpoint.accept().await.unwrap().await.unwrap()});
        let dc=device.connect(hub.addr(),ALPN).await.unwrap();let hc=incoming.await.unwrap();
        let ds=Session::new(dc,device.id(),Role::Device).unwrap();let hs=Session::new(hc,hub.id(),Role::Hub).unwrap();
        let receiver=Arc::new(PushReceiver{h:h.clone(),root:quarantine.clone(),pause:None,validated:AtomicUsize::new(0)});
        let _hub_job=tokio::spawn(hs.clone().serve(receiver.clone()));
        ds.request(json!({"method":"hub.enroll","target":null,"correlation_id":"ab".repeat(16),"timeout_ms":5000,"params":{"offer":{"versions":["0.1"]},"invite":null}})).await.unwrap();
        let _device_job=tokio::spawn(ds.serve(receiver.clone()));
        let reply=hs.push_git(request,&mut bytes.as_slice()).await.unwrap();assert_eq!(reply["result"]["state"],"queued");
        assert_eq!(receiver.validated.load(Ordering::SeqCst),1);
        assert_eq!(h.count("proto_ref_audit"),1);assert_eq!(h.count("proto_git_operations"),1);
        if recovery {
            assert_eq!(Some(reply["result"].clone()),recovered);
            assert_eq!(reply,serde_json::from_slice::<Value>(&fs::read(path.join("original-reply.json")).unwrap()).unwrap());
        } else {fs::write(path.join("original-reply.json"),serde_json::to_vec(&reply).unwrap()).unwrap();}
        assert_quarantines(&quarantine,0).await;
        // Abrupt process exit: no Session/Authority/endpoint destructors run.
        std::process::exit(if recovery {72}else{71});
    }).await.unwrap();
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
        let winner=outcomes.iter().find_map(|r|r.as_ref().ok().filter(|v|v["result"]["state"]=="queued")).unwrap().clone();
        let mut retry=request.clone();
        retry["correlation_id"]=winner["correlation_id"].clone();
        retry["timeout_ms"]=json!(10000);
        let session=hs.clone();let r=retry.clone();let b=bytes.clone();
        let repeat=tokio::spawn(async move {session.push_git(r,&mut b.as_slice()).await});
        gate.entered.acquire().await.unwrap().forget();
        gate.release.add_permits(1);
        assert_eq!(repeat.await.unwrap().unwrap(),winner,"same intent, new timeout returns original durable receipt");
        assert_eq!(receiver.validated.load(Ordering::SeqCst),3,"duplicate fully validates again");
        assert_eq!(h.count("proto_git_operations"),1);
        assert_eq!(h.count("proto_ref_audit"),1);
        let mut changed=retry.clone();changed["params"]["sha256"]=json!("ee".repeat(32));
        assert!(hs.push_git(changed,&mut NeverRead).await.is_err(),"changed intent conflicts before credit");
        assert!(hs.push_git(make_request("d1",&bytes,&metadata),&mut NeverRead).await.is_err(),"new operation signed replay denies before credit/source read");
        assert_eq!(h.count("proto_ref_audit"),1);
        hub_job.abort(); device_job.abort(); let _=hub_job.await; let _=device_job.await;
        hub.close().await; device.close().await;
    }).await.unwrap();
}
