use coglet_hub::{ActionGrant, EventGrant, Kernel, Registration};
#[path = "support/hardening.rs"]
mod hardening;
use coglet_proto::{
    authority::Grant,
    signing,
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

#[tokio::test]
async fn new_dispatch_requires_live_runtime_clock_before_attempt_marking() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir().unwrap();
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
        let actor = format!("hub:{}", hub.id());
        let source = format!("surface:{}", SecretKey::generate().public());
        let r = registration(&SecretKey::generate(), hub.id(), device.id(), &source, &actor);
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite = kernel.invite(device.id(), Duration::from_secs(60), vec![Grant {method:"hub.actions".into(), target:Some(r.coglet.clone()), expires:10000, generation:0}]).await.unwrap();
        let (ds, hs, serving) = connect(&device, &hub, kernel.clone()).await;
        let enroll = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
        assert_eq!(ds.request(enroll).await.unwrap()["type"], "reply");
        kernel.register(r.clone()).await.unwrap();
        let id = "44".repeat(16);
        let action = json!({"id":id,"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
        kernel.prepare_action(&r.coglet, action, json!({"actor":actor}), Some(0)).await.unwrap();
        let receiver = Arc::new(MockSurface(Mutex::new(vec![])));
        let responder = tokio::spawn(ds.serve(receiver.clone()));
        assert!(kernel.dispatch_action(&r.coglet, &id, hs).await.is_err(), "new action dispatched without trusted logical time");
        assert!(receiver.0.lock().unwrap().is_empty());
        let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
        let attempted: i64 = db.query_row("SELECT attempted FROM deliveries", [], |r| r.get(0)).unwrap();
        assert_eq!(attempted, 0, "pre-I/O clock refusal poisoned retained action");
        responder.abort(); serving.abort(); device.close().await; hub.close().await;
    }).await.unwrap();
}

// Independent real-network fixtures share host CPU/disk; serialize this test
// binary without weakening any per-dispatch/control latency assertion.
static SERIAL: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

struct StalledSurface(tokio::sync::Notify);
impl OperationHandler for StalledSurface {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            self.0.notify_one();
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn stalled_dispatch_stops_when_trusted_authority_expires() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let now = Arc::new(AtomicU64::new(1));
        let clock = {let now = now.clone(); Arc::new(move || Some(now.load(Ordering::SeqCst)))};
        let kernel = Kernel::open(dir.path(), hub.id(), clock).unwrap();
        let actor = format!("hub:{}", hub.id());
        let source = format!("surface:{}", SecretKey::generate().public());
        let r = registration(&SecretKey::generate(), hub.id(), device.id(), &source, &actor);
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite = kernel.invite(device.id(), Duration::from_secs(60), vec![Grant {method:"hub.actions".into(), target:Some(r.coglet.clone()), expires:10000, generation:0}]).await.unwrap();
        let (ds, hs, serving) = connect(&device, &hub, kernel.clone()).await;
        let enroll = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
        assert_eq!(ds.request(enroll).await.unwrap()["type"], "reply");
        kernel.register(r.clone()).await.unwrap();
        let action = json!({"id":"44".repeat(16),"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
        kernel.prepare_action(&r.coglet, action, json!({"actor":actor}), Some(0)).await.unwrap();
        let stalled = Arc::new(StalledSurface(tokio::sync::Notify::new()));
        let responder = tokio::spawn(ds.serve(stalled.clone()));
        let retry_session = hs.clone();
        let dispatch = tokio::spawn({let kernel=kernel.clone();let target=r.coglet.clone();async move {kernel.dispatch_action_with_clock(&target,&"44".repeat(16),hs,Arc::new(||Some(0))).await}});
        stalled.0.notified().await;
        let mut extra_dispatches=Vec::new();
        for n in 1..8u8 {
            let id=format!("{n:02x}").repeat(16);
            let action=json!({"id":id,"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
            kernel.prepare_action(&r.coglet,action,json!({"actor":actor}),Some(0)).await.unwrap();
            extra_dispatches.push(tokio::spawn({let kernel=kernel.clone();let target=r.coglet.clone();let session=retry_session.clone();async move {kernel.dispatch_action_with_clock(&target,&id,session,Arc::new(||Some(0))).await}}));
            stalled.0.notified().await;
        }
        let ninth="99".repeat(16);
        let action=json!({"id":ninth,"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
        kernel.prepare_action(&r.coglet,action,json!({"actor":actor}),Some(0)).await.unwrap();
        assert!(matches!(kernel.dispatch_action_with_clock(&r.coglet,&ninth,retry_session,Arc::new(||Some(0))).await,Err(coglet_hub::Error::Full)),"capacity saturation poisoned an unsent action");
        let db=rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
        let retained:(i64,Option<String>)=db.query_row("SELECT attempted,reply FROM deliveries WHERE id=?1",[ninth],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(retained,(0,None));
        now.store(10000, Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_millis(500), dispatch).await.expect("expired authority did not fence pending transport").unwrap().unwrap();
        assert_eq!(result["error"]["details"]["unknown_effect"], true);
        for task in extra_dispatches {tokio::time::timeout(Duration::from_millis(500),task).await.unwrap().unwrap().unwrap();}
        responder.abort(); serving.abort(); device.close().await; hub.close().await;
    }).await.unwrap();
}

#[tokio::test]
async fn stalled_peer_does_not_block_other_peer_or_its_own_revocation() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let a = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let b = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
        let actor = format!("hub:{}", hub.id());
        let source = format!("surface:{}", SecretKey::generate().public());
        let ra = registration(&SecretKey::generate(), hub.id(), a.id(), &source, &actor);
        let rb = registration(&SecretKey::generate(), hub.id(), b.id(), &source, &actor);
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let mut sessions = vec![]; let mut tasks = vec![];
        for (device, r) in [(&a, &ra), (&b, &rb)] {
            let grants = ["hub.actions","hub.events"].into_iter().map(|method| Grant { method: method.into(), target: Some(r.coglet.clone()), expires: 10000, generation: 0 }).collect();
            let invite = kernel.invite(device.id(), Duration::from_secs(60), grants).await.unwrap();
            let (ds, hs, task) = connect(device, &hub, kernel.clone()).await;
            let enroll = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
            assert_eq!(ds.request(enroll).await.unwrap()["type"], "reply");
            kernel.register(r.clone()).await.unwrap();
            let action = json!({"id":"44".repeat(16),"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
            kernel.prepare_action(&r.coglet, action, json!({"actor":actor}), Some(0)).await.unwrap();
            sessions.push((ds, hs)); tasks.push(task);
        }
        let stalled = Arc::new(StalledSurface(tokio::sync::Notify::new()));
        tasks.push(tokio::spawn({ let s=sessions[0].0.clone(); let handler=stalled.clone(); async move {let _=s.serve(handler).await;} }));
        let other = Arc::new(MockSurface(Mutex::new(vec![])));
        tasks.push(tokio::spawn({let s=sessions[1].0.clone(); async move {let _=s.serve(other).await;} }));
        let dispatch = tokio::spawn({let k=kernel.clone();let t=ra.coglet.clone();let s=sessions[0].1.clone();async move {k.dispatch_action_with_clock(&t,&"44".repeat(16),s,Arc::new(||Some(0))).await}});
        stalled.0.notified().await;
        // Device-to-hub outcome stream can overtake the action-accept stream.
        let early=json!({"method":"hub.events","target":ra.coglet,"timeout_ms":1000,"correlation_id":"77".repeat(16),"params":{"offset":0,"events":[{"source":ra.coglet,"kind":"action.outcome","seq":1,"logical_time":1,"correlation_id":"22".repeat(16),"payload":{"id":"44".repeat(16),"correlation_id":"22".repeat(16),"status":"succeeded","result":{}}}]}});
        assert_eq!(sessions[0].0.request(early).await.unwrap()["result"]["next_offset"],1,"valid outcome overtook action-accept");
        let duplicate=tokio::time::timeout(Duration::from_millis(500),kernel.dispatch_action_with_clock(&ra.coglet,&"44".repeat(16),sessions[0].1.clone(),Arc::new(||Some(0)))).await.expect("duplicate live attempt queried a receipt before admission");
        assert!(duplicate.is_err());
        let result = tokio::time::timeout(Duration::from_millis(500), kernel.dispatch_action_with_clock(&rb.coglet, &"44".repeat(16), sessions[1].1.clone(),Arc::new(||Some(0)))).await.expect("stalled A blocked B").unwrap();
        assert_eq!(result["result"]["accepted"], true);
        tokio::time::timeout(Duration::from_millis(500), kernel.revoke_peer(a.id())).await.expect("stalled A blocked revoke A").unwrap();
        let cancelled = tokio::time::timeout(Duration::from_millis(500), dispatch).await.expect("revocation did not cancel pending dispatch").unwrap();
        assert_eq!(cancelled.unwrap()["error"]["details"]["unknown_effect"],false,"durable outcome already establishes the effect");
        for task in tasks {task.abort();}
        a.close().await; b.close().await; hub.close().await;
    }).await.unwrap();
}

struct Receipts(Mutex<Vec<Value>>);
impl OperationHandler for Receipts {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        r: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(r.clone());
            assert_eq!(
                r["method"], "hub.receipt",
                "ambiguous action was blindly replayed"
            );
            if r["params"]["operation_id"] == "44".repeat(16) {
                json!({"type":"reply","correlation_id":r["correlation_id"],"result":{"operation_id":r["params"]["operation_id"],"state":"applied","step":1,"error":null}})
            } else {
                json!({"type":"error","correlation_id":r["correlation_id"],"error":{"code":"not_found","message":"not found","retryable":false,"details":{}}})
            }
        })
    }
}
#[tokio::test]
async fn crashed_attempt_queries_original_scope_receipt_and_never_replays_unknown_effects() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir=tempfile::tempdir().unwrap();
        let hub=Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device=Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let kernel=Kernel::open(dir.path(),hub.id(),Arc::new(||Some(1))).unwrap();
        let actor=format!("hub:{}",hub.id()); let ticket="aa".repeat(16);
        let source=format!("surface:{}",SecretKey::generate().public());
        let mut r=registration(&SecretKey::generate(),hub.id(),device.id(),&source,&actor);
        r.actions[0].ticket=Some(ticket.clone());
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite=kernel.invite(device.id(),Duration::from_secs(60),["hub.actions","hub.events"].into_iter().map(|method| Grant{method:method.into(),target:Some(r.coglet.clone()),expires:10000,generation:0}).collect()).await.unwrap();
        let (ds,hs,serving)=connect(&device,&hub,kernel.clone()).await;
        let enroll=json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
        assert_eq!(ds.request(enroll).await.unwrap()["type"],"reply"); kernel.register(r.clone()).await.unwrap();
        for id in ["44".repeat(16),"55".repeat(16)] {
            let action=json!({"id":id,"coglet":r.coglet,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
            kernel.prepare_action(&r.coglet,action,json!({"actor":actor,"ticket":ticket}),Some(100)).await.unwrap();
        }
        let db=rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
        db.execute("UPDATE deliveries SET attempted=1",[]).unwrap();
        let receiver=Arc::new(Receipts(Mutex::new(vec![])));
        let responder=tokio::spawn(ds.clone().serve(receiver.clone()));
        let result=kernel.dispatch_action(&r.coglet,&"44".repeat(16),hs.clone()).await.unwrap();
        assert_eq!(result["result"]["state"],"applied");
        let failed=kernel.dispatch_action(&r.coglet,&"55".repeat(16),hs.clone()).await.unwrap();
        assert_eq!(failed["error"]["code"],"unavailable");
        assert_eq!(failed["error"]["details"]["unknown_effect"],true);
        assert_eq!(kernel.dispatch_action(&r.coglet,&"55".repeat(16),hs).await.unwrap(),failed);
        // Local uncertainty is not an authoritative runtime rejection. Reliable
        // outcomes arriving after the uncertain observation must still be admitted.
        let late=json!({"method":"hub.events","target":r.coglet,"timeout_ms":1000,"correlation_id":"77".repeat(16),"params":{"offset":0,"events":[{"source":r.coglet,"kind":"action.outcome","seq":1,"logical_time":1,"correlation_id":"22".repeat(16),"payload":{"id":"55".repeat(16),"correlation_id":"22".repeat(16),"status":"succeeded","result":{}}}]}});
        assert_eq!(ds.request(late.clone()).await.unwrap()["result"]["next_offset"],1,"late authoritative outcome was blocked by local uncertainty");
        assert_eq!(ds.request(late).await.unwrap()["result"]["next_offset"],1);
        assert_eq!(kernel.events(&r.coglet).await.unwrap().len(),1);
        {let calls=receiver.0.lock().unwrap(); assert_eq!(calls.len(),2);
        for call in calls.iter() {assert_eq!(call["target"],r.coglet);assert_eq!(call["context"],json!({"actor":actor,"ticket":ticket}));}}
        responder.abort(); serving.abort(); device.close().await; hub.close().await;
    }).await.unwrap();
}

struct MockSurface(Mutex<Vec<Value>>);
impl OperationHandler for MockSurface {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        r: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(r.clone());
            json!({"type":"reply","correlation_id":r["correlation_id"],"result":{"id":r["params"]["action"]["id"],"correlation_id":r["params"]["action"]["correlation_id"],"accepted":true}})
        })
    }
}
fn registration(
    key: &SecretKey,
    hub: iroh::EndpointId,
    device: iroh::EndpointId,
    source: &str,
    actor: &str,
) -> Registration {
    let coglet = format!("coglet:{}", key.public());
    let statement = json!({"purpose":"delegation","hub":hub.to_string(),"device":device.to_string(),"coglet":coglet,"generation":1,"expires":10000,"methods":["hub.events","hub.actions"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    Registration {
        coglet,
        device: device.to_string(),
        grant_generation: 0,
        delegation: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
        events: vec![EventGrant {
            source: source.into(),
            kind: "test.tick".into(),
        }],
        actions: vec![ActionGrant {
            actor: actor.into(),
            ticket: None,
            capability: "test.move".into(),
        }],
    }
}
async fn connect(
    device: &Endpoint,
    hub: &Endpoint,
    kernel: Kernel,
) -> (Session, Session, tokio::task::JoinHandle<()>) {
    let h = hub.clone();
    let accept = tokio::spawn(async move {
        Session::new(h.accept().await.unwrap().await.unwrap(), h.id(), Role::Hub).unwrap()
    });
    let device_session = Session::new(
        device.connect(hub.addr(), ALPN).await.unwrap(),
        device.id(),
        Role::Device,
    )
    .unwrap();
    let hub_session = accept.await.unwrap();
    let serve_session = hub_session.clone();
    let serve = tokio::spawn(async move {
        let _ = serve_session.serve(Arc::new(kernel)).await;
    });
    (device_session, hub_session, serve)
}
#[tokio::test]
async fn two_devices_enroll_event_reaches_coglet_and_mapped_action_reaches_other_surface() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(20),async {
        let dir=tempfile::tempdir().unwrap();
        let hub=Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let a=Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let b=Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let kernel=Kernel::open(dir.path(),hub.id(),Arc::new(||Some(1))).unwrap();
        let actor=format!("hub:{}",hub.id()); let source=format!("surface:{}",SecretKey::generate().public());
        let ra=registration(&SecretKey::generate(),hub.id(),a.id(),&source,&actor);
        let rb=registration(&SecretKey::generate(),hub.id(),b.id(),&source,&actor);
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let mut tasks=Vec::new(); let mut sessions=Vec::new();
        for (device,r) in [(&a,&ra),(&b,&rb)] {
            let grants=["hub.events","hub.actions"].into_iter().map(|m|Grant{method:m.into(),target:Some(r.coglet.clone()),expires:10000,generation:0}).collect();
            let invite=kernel.invite(device.id(),Duration::from_secs(60),grants).await.unwrap();
            let (ds,hs,task)=connect(device,&hub,kernel.clone()).await;
            let enroll=json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
            assert_eq!(ds.request(enroll).await.unwrap()["type"],"reply");
            kernel.register(r.clone()).await.unwrap();
            tasks.push(task); sessions.push((ds,hs));
        }
        let event=json!({"source":source,"kind":"test.tick","seq":1,"logical_time":0,"correlation_id":"22".repeat(16),"payload":{"x":4}});
        let request=json!({"method":"hub.events","target":ra.coglet,"timeout_ms":1000,"correlation_id":"33".repeat(16),"params":{"offset":0,"events":[event.clone()]}});
        assert_eq!(sessions[0].0.request(request.clone()).await.unwrap()["result"]["next_offset"],1);
        assert_eq!(sessions[0].0.request(request).await.unwrap()["result"]["next_offset"],1);
        // Local hub coglet adapter consumes the exact admitted envelope; its decision
        // is test logic, not a product-specific strategy embedded in the kernel.
        let input=kernel.events(&ra.coglet).await.unwrap(); assert_eq!(input,vec![event]);
        let action=json!({"id":"44".repeat(16),"coglet":rb.coglet,"capability":"test.move","args":{"x":input[0]["payload"]["x"]},"deadline":200,"correlation_id":"22".repeat(16)});
        let mapped=kernel.prepare_action(&rb.coglet,action.clone(),json!({"actor":actor}),Some(1000)).await.unwrap();
        assert_eq!(mapped["params"]["action"]["deadline"],1200);
        assert_eq!(kernel.prepare_action(&rb.coglet,action,json!({"actor":actor}),None).await.unwrap(),mapped);
        let surface=Arc::new(MockSurface(Mutex::new(vec![])));
        let serving=tokio::spawn(sessions[1].0.clone().serve(surface.clone()));
        let result=kernel.dispatch_action_with_clock(&rb.coglet,&"44".repeat(16),sessions[1].1.clone(),Arc::new(||Some(1000))).await.unwrap();
        assert_eq!(result["result"]["accepted"],true);
        assert_eq!(surface.0.lock().unwrap().as_slice(),&[mapped]);
        kernel.dispatch_action_with_clock(&rb.coglet,&"44".repeat(16),sessions[1].1.clone(),Arc::new(||Some(1000))).await.unwrap();
        assert_eq!(surface.0.lock().unwrap().len(),1);
        let outcome=json!({"source":rb.coglet,"kind":"action.outcome","seq":1,"logical_time":1,"correlation_id":"22".repeat(16),"payload":{"id":"44".repeat(16),"correlation_id":"22".repeat(16),"status":"succeeded","result":{}}});
        let outcome_request=json!({"method":"hub.events","target":rb.coglet,"timeout_ms":1000,"correlation_id":"55".repeat(16),"params":{"offset":0,"events":[outcome.clone()]}});
        for (field,value) in [("id",json!("99".repeat(16))),("correlation_id",json!("99".repeat(16)))] {
            let mut forged=outcome_request.clone(); forged["params"]["events"][0]["payload"][field]=value;
            assert_eq!(sessions[1].0.request(forged).await.unwrap()["error"]["code"],"permission_denied");
        }
        assert_eq!(sessions[1].0.request(outcome_request.clone()).await.unwrap()["result"]["next_offset"],1);
        assert_eq!(sessions[1].0.request(outcome_request).await.unwrap()["result"]["next_offset"],1);
        assert_eq!(kernel.events(&rb.coglet).await.unwrap(),vec![outcome.clone()]);
        // Same action cannot acquire a second outcome at a new source sequence or
        // relay offset, even after its payload was consumed by the local adapter.
        let deliveries=kernel.event_deliveries(&rb.coglet).await.unwrap();
        kernel.ack_events(&rb.coglet,deliveries[0].0).await.unwrap();
        let mut repeated=outcome; repeated["seq"]=json!(2);
        let again=json!({"method":"hub.events","target":rb.coglet,"timeout_ms":1000,"correlation_id":"66".repeat(16),"params":{"offset":1,"events":[repeated]}});
        assert_eq!(sessions[1].0.request(again).await.unwrap()["error"]["code"],"conflict");
        let audit=kernel.audit().await.unwrap();
        for method in ["hub.enroll","hub.events","hub.actions","hub.actions.dispatch"] { assert!(audit.iter().any(|r|r["method"]==method),"missing {method}"); }
        serving.abort(); for task in tasks { task.abort(); } a.close().await; b.close().await; hub.close().await;
    }).await.unwrap();
}
