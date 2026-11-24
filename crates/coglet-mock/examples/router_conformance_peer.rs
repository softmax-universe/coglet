//! Shared isolated fixtures for plain Kernel and coglet-mock::hub. No fixture
//! method is installed on the ALPN; every request traverses encrypted Session.
use coglet_hub::{EventGrant, Kernel, Registration, Store};
use coglet_mock::hub::{MockHub, Result};
use coglet_proto::{
    authority::{Authority, AuthorityError, Grant},
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
    wire,
};
use iroh::{Endpoint, EndpointId, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, sync::Mutex, task::JoinSet};

#[path = "support/router_fetch.rs"]
mod router_fetch;

type Sessions = Arc<Mutex<HashMap<EndpointId, (u64, Session)>>>;

/// Reference adapter only supplies transport around the production Kernel. The
/// alternate backend exercises MockHub's independently owned listener/session
/// lifecycle. Both retain the same production policy and fixture provisioning.
struct Reference {
    endpoint: Endpoint,
    kernel: Kernel,
    sessions: Sessions,
    task: tokio::task::JoinHandle<()>,
    state: tempfile::TempDir,
}
impl Reference {
    async fn start() -> Result<Self> {
        let state = tempfile::tempdir()?;
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::from_bytes(&[2; 32]))
            .alpns(vec![ALPN.to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>()?)?
            .bind()
            .await?;
        let root = state.path().to_owned();
        let id = endpoint.id();
        let kernel = tokio::task::spawn_blocking(move || {
            Kernel::open(root, id, Arc::new(|| Some(1_800_000_000)))
        })
        .await??;
        let sessions = Sessions::default();
        let active = sessions.clone();
        let server = endpoint.clone();
        let handler = kernel.clone();
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                        // Session shutdown is normal; a task panic is not.
                        if let Err(error) = result { panic!("reference connection task: {error}"); }
                    }
                    incoming = server.accept() => {
                        let Some(incoming) = incoming else { break };
                        if tasks.len() >= 16 { drop(incoming); continue; }
                        let kernel = handler.clone();
                        let sessions = active.clone();
                        let local = server.id();
                        tasks.spawn(async move {
                            let Ok(Ok(connection)) = tokio::time::timeout(Duration::from_secs(5), incoming).await else { return };
                            let Ok(session) = Session::new(connection, local, Role::Hub) else { return };
                            let peer = session.peer();
                            let Ok(token) = kernel.transport_connected(peer).await else { return };
                            sessions.lock().await.insert(peer, (token, session.clone()));
                            let _ = session.serve(Arc::new(kernel.clone())).await;
                            let mut active = sessions.lock().await;
                            if active.get(&peer).is_some_and(|(current, _)| *current == token) { active.remove(&peer); }
                            drop(active);
                            let _ = kernel.transport_disconnected(peer, token).await;
                        });
                    }
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        });
        Ok(Self {
            endpoint,
            kernel,
            sessions,
            task,
            state,
        })
    }
}
impl Drop for Reference {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum Backend {
    Reference(Reference),
    Mock(MockHub),
}
impl Backend {
    async fn start() -> Result<Self> {
        match std::env::var("COGLET_FIXTURE_IMPLEMENTATION")?.as_str() {
            "reference" => Ok(Self::Reference(Reference::start().await?)),
            "mock" => Ok(Self::Mock(MockHub::start().await?)),
            _ => Err("unknown router implementation".into()),
        }
    }
    fn kernel(&self) -> &Kernel {
        match self {
            Self::Reference(r) => &r.kernel,
            Self::Mock(m) => m.kernel(),
        }
    }
    fn addr(&self) -> iroh::EndpointAddr {
        match self {
            Self::Reference(r) => r.endpoint.addr(),
            Self::Mock(m) => m.addr(),
        }
    }
    fn state_path(&self) -> &std::path::Path {
        match self {
            Self::Reference(r) => r.state.path(),
            Self::Mock(m) => m.state_path(),
        }
    }
    async fn session(&self, device: EndpointId) -> Option<Session> {
        match self {
            Self::Reference(r) => r.sessions.lock().await.get(&device).map(|(_, s)| s.clone()),
            Self::Mock(m) => m.session(device).await,
        }
    }
    async fn close(self) -> Result<()> {
        match self {
            Self::Reference(mut r) => {
                r.endpoint.close().await;
                tokio::time::timeout(Duration::from_secs(5), &mut r.task).await??;
            }
            Self::Mock(m) => m.shutdown().await,
        }
        Ok(())
    }
}

struct UnselectedDevice;
impl OperationHandler for UnselectedDevice {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async { panic!("pre-enrollment request reached device policy") })
    }
}

fn expected_events(case: &str) -> Option<usize> {
    match case {
        "event-batch-dedup"
        | "event-offset-conflict"
        | "event-source-repeat"
        | "event-empty-probe" => Some(1),
        "event-source-gap" | "event-retry-a-after-b" => Some(2),
        "event-gap"
        | "event-runtime-wake"
        | "event-coglet-forged-event"
        | "event-foreign-coglet" => Some(0),
        _ => None,
    }
}

fn enrollment_supported(case: &str) -> bool {
    matches!(
        case,
        "enroll-reconnect"
            | "health-minimal"
            | "enroll-unsupported-version"
            | "enroll-one-use"
            | "enroll-expired"
            | "enroll-window-closed"
            | "enroll-wrong-node"
            | "start-before-enroll"
    )
}

async fn enroll(session: &Session, token: Option<&str>, id: &str) -> Result<()> {
    let reply = session
        .request(json!({"method":"hub.enroll", "target":null,
        "timeout_ms":2000, "correlation_id":id.repeat(16),
        "params":{"offer":{"versions":["0.1"]},"invite":token}}))
        .await?;
    if reply["type"] != "reply" {
        return Err("fixture enrollment failed".into());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let case = std::env::var("COGLET_CONFORMANCE_CASE")?;
    if router_fetch::supported(&case) {
        return router_fetch::run(Backend::start().await?, &case).await;
    }
    let expected = expected_events(&case);
    if expected.is_none() && !enrollment_supported(&case) {
        return Err("unsupported isolated router fixture".into());
    }
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(format!("conformance/hub/{case}.json"))?)?;
    let requests = fixture["steps"]
        .as_array()
        .ok_or("bad case")?
        .iter()
        .filter(|step| step.get("send").is_some())
        .count();
    // Independent expected admitted request indices, not inferred from replies.
    // Each frozen admitted batch here contains one event; the empty probe and
    // rejected/retried batches must not add either payloads or admission audits.
    let admitted: Vec<usize> = match case.as_str() {
        "event-empty-probe" => vec![1],
        "event-source-gap" | "event-retry-a-after-b" => vec![0, 1],
        _ if expected == Some(1) => vec![0],
        _ => vec![],
    };
    let sends: Vec<_> = fixture["steps"]
        .as_array()
        .ok_or("bad steps")?
        .iter()
        .filter_map(|step| step.get("send"))
        .collect();
    let expected_payloads: Vec<Value> = admitted
        .iter()
        .flat_map(|index| {
            sends[*index]["params"]["events"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect();
    let router = Backend::start().await?;
    let device = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[3; 32]))
        .bind()
        .await?;
    let mut target = None;
    let mut grants = vec![Grant {
        method: "hub.health".into(),
        target: None,
        expires: u64::MAX,
        generation: 0,
    }];
    if expected.is_some() {
        let signatures: Value =
            serde_json::from_slice(&std::fs::read("spec/fixtures/hub/signatures.json")?)?;
        let mut delegation = signatures[0].clone();
        delegation
            .as_object_mut()
            .ok_or("bad delegation")?
            .remove("preimage_hex");
        let coglet = delegation["statement"]["coglet"]
            .as_str()
            .ok_or("bad target")?
            .to_owned();
        grants.push(Grant {
            method: "hub.events".into(),
            target: Some(coglet.clone()),
            expires: 4102444800,
            generation: 0,
        });
        router
            .kernel()
            .register(Registration {
                coglet: coglet.clone(),
                device: device.id().to_string(),
                grant_generation: 0,
                delegation,
                events: vec![EventGrant {
                    source: format!("surface:{}", device.id()),
                    kind: "test.event".into(),
                }],
                actions: vec![],
            })
            .await?;
        target = Some(coglet);
    }
    router.kernel().open_window(Duration::from_secs(60)).await?;
    let peer = if case == "enroll-wrong-node" {
        SecretKey::from_bytes(&[4; 32]).public()
    } else {
        device.id()
    };
    let token = "11".repeat(32);
    let ttl = if case == "enroll-expired" {
        Duration::from_millis(1)
    } else {
        Duration::from_secs(60)
    };
    router
        .kernel()
        .test_invite(peer, token.clone(), ttl, grants)
        .await?;
    if case == "enroll-expired" {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if case == "enroll-window-closed" {
        router.kernel().test_close_window().await?;
    }
    if matches!(case.as_str(), "enroll-reconnect" | "health-minimal") {
        let conn = device.connect(router.addr(), ALPN).await?;
        let session = Session::new(conn.clone(), device.id(), Role::Device)?;
        enroll(&session, Some(&token), "ff").await?;
        conn.close(0u32.into(), b"fixture reconnect");
        conn.closed().await;
    }
    let mut session = Session::new(
        device.connect(router.addr(), ALPN).await?,
        device.id(),
        Role::Device,
    )?;
    let mut reverse = None;
    if case == "start-before-enroll" {
        reverse = Some(tokio::spawn(
            session.clone().serve(Arc::new(UnselectedDevice)),
        ));
        session = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(s) = router.session(device.id()).await {
                    break s;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
    } else if case == "health-minimal" {
        enroll(&session, None, "fe").await?;
    } else if expected.is_some() {
        enroll(&session, Some(&token), "ff").await?;
    }
    eprintln!("COGLET_ROUTER_READY");
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    for index in 0..requests {
        let request = wire::read_frame(&mut input).await?;
        let reply = session.request(request).await?;
        if index + 1 == requests {
            let path = router.state_path().join("authority.db");
            let peer = device.id();
            let enrolled = expected.is_some()
                || matches!(
                    case.as_str(),
                    "enroll-reconnect" | "health-minimal" | "enroll-one-use"
                );
            tokio::task::spawn_blocking(move || -> Result<()> {
                let authority = Authority::open(path)?;
                // Null enrollment on an independently reopened Authority is a
                // read-only allowlist recovery check, not fixture provisioning.
                match authority.enroll(peer, None) {
                    Ok(()) if enrolled => Ok(()),
                    Err(AuthorityError::Unauthenticated) if !enrolled => Ok(()),
                    _ => Err("durable enrollment oracle failed".into()),
                }
            })
            .await??;
        }
        if index + 1 == requests
            && let Some(count) = expected
        {
            // Reopen independently: reply matches cannot establish once-only
            // relay admission or the absence of effects from rejected batches.
            let path = router.state_path().join("kernel.db");
            let target = target.clone().ok_or("missing event target")?;
            let payloads = expected_payloads.clone();
            let expected_audits: Vec<Value> = admitted
                .iter()
                .map(|index| {
                    let params = &sends[*index]["params"];
                    let offset = params["offset"].as_u64().unwrap();
                    json!({"method":"hub.events", "peer":device.id().to_string(),
                       "target":target,"offset":offset,
                       "next_offset":offset + params["events"].as_array().unwrap().len() as u64,
                       "outcome":"admitted"})
                })
                .collect();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let store = Store::open(path)?;
                if payloads.len() != count || store.pending_events(&target)? != payloads {
                    return Err("durable event oracle failed".into());
                }
                let audited: Vec<Value> = store
                    .audit()?
                    .into_iter()
                    .filter(|row| row["method"] == "hub.events")
                    .collect();
                if audited != expected_audits {
                    return Err("durable event audit oracle failed".into());
                }
                Ok(())
            })
            .await??;
        }
        wire::write_frame(&mut output, &reply).await?;
        output.flush().await?;
    }
    device.close().await;
    if let Some(task) = reverse {
        let _ = task.await?;
    }
    router.close().await?;
    Ok(())
}
