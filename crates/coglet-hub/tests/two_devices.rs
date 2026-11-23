use coglet_hub::{ActionGrant, EventGrant, Kernel, Registration};
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
        let result=kernel.dispatch_action(&rb.coglet,&"44".repeat(16),sessions[1].1.clone()).await.unwrap();
        assert_eq!(result["result"]["accepted"],true);
        assert_eq!(surface.0.lock().unwrap().as_slice(),&[mapped]);
        kernel.dispatch_action(&rb.coglet,&"44".repeat(16),sessions[1].1.clone()).await.unwrap();
        assert_eq!(surface.0.lock().unwrap().len(),1);
        let outcome=json!({"source":rb.coglet,"kind":"action.outcome","seq":1,"logical_time":1,"correlation_id":"22".repeat(16),"payload":{"id":"44".repeat(16),"correlation_id":"22".repeat(16),"status":"succeeded","result":{}}});
        let outcome_request=json!({"method":"hub.events","target":rb.coglet,"timeout_ms":1000,"correlation_id":"55".repeat(16),"params":{"offset":0,"events":[outcome.clone()]}});
        assert_eq!(sessions[1].0.request(outcome_request.clone()).await.unwrap()["result"]["next_offset"],1);
        assert_eq!(sessions[1].0.request(outcome_request).await.unwrap()["result"]["next_offset"],1);
        assert_eq!(kernel.events(&rb.coglet).await.unwrap(),vec![outcome]);
        let audit=kernel.audit().await.unwrap();
        for method in ["hub.enroll","hub.events","hub.actions","hub.actions.dispatch"] { assert!(audit.iter().any(|r|r["method"]==method),"missing {method}"); }
        serving.abort(); for task in tasks { task.abort(); } a.close().await; b.close().await; hub.close().await;
    }).await.unwrap();
}
