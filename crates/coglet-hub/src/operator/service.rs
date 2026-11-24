//! Production trusted-local provisioning. No remote or implicit actor authority.
use super::{
    PeerIdentity, Request,
    server::{Handler, Reply},
};
use crate::kernel::saga::Executor;
use crate::{Error, Kernel, Result};
use coglet_proto::transport::Session;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
};
type Observers = BTreeMap<String, tokio::sync::watch::Sender<Value>>;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantPolicy {
    pub peer: String,
    pub method: String,
    pub target: Option<String>,
    pub max_expires_unix_s: u64,
}
/// Trusted startup configuration, never populated from socket input. Authority
/// is non-ticket only; no wildcard operations, methods, targets or peers.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provisioning {
    pub uid: u32,
    pub actor: String,
    pub operations: Vec<String>,
    pub max_window_ms: u64,
    pub grants: Vec<GrantPolicy>,
}
impl Provisioning {
    pub fn validate(&self, service_uid: u32) -> Result<()> {
        if self.uid != service_uid
            || !super::valid("source-id", &json!(self.actor))
            || self.max_window_ms > 300000
            || self.operations.len() > 9
            || self.grants.len() > 256
            || self.operations.iter().enumerate().any(|(i, op)| {
                !matches!(
                    op.as_str(),
                    "grant.provision"
                        | "grant.revoke"
                        | "enrollment.window"
                        | "enrollment.invite"
                        | "deploy"
                        | "push"
                        | "stop"
                        | "status"
                ) || self.operations[..i].contains(op)
            })
            || self.grants.iter().any(|g| {
                g.peer.parse::<iroh::EndpointId>().is_err()
                    || !super::valid("hub-method", &json!(g.method))
                    || !super::valid("hub-target", &json!(g.target))
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub(crate) fn permits(&self, peer: PeerIdentity, request: &Request) -> bool {
        let r = request.value();
        if peer.uid != self.uid || !self.operations.iter().any(|op| r["op"] == *op) {
            return false;
        }
        let p = &r["params"];
        match r["op"].as_str() {
            Some("grant.provision" | "grant.revoke") => self.permits_grant(p),
            Some("enrollment.window") => p["duration_ms"]
                .as_u64()
                .is_some_and(|n| n <= self.max_window_ms),
            Some("enrollment.invite") => {
                self.grants.iter().any(|g| p["expected_device"] == g.peer)
                    && p["grants"]
                        .as_array()
                        .is_some_and(|gs| gs.iter().all(|g| self.permits_grant(g)))
            }
            _ => false, // A qualified saga owns downstream dispatch, never this configuration handler.
        }
    }
    fn permits_grant(&self, p: &Value) -> bool {
        self.grants.iter().any(|g| {
            p["peer"] == g.peer
                && p["method"] == g.method
                && p["target"] == json!(g.target)
                && p.get("expires_unix_s")
                    .is_none_or(|v| v.as_u64().is_some_and(|n| n <= g.max_expires_unix_s))
        })
    }
}
pub struct Service {
    kernel: Kernel,
    provisioning: Option<Provisioning>,
    executor: Option<Executor>,
    sessions: Mutex<BTreeMap<String, (u64, Session)>>,
    observers: Mutex<Observers>,
    wake: tokio::sync::Notify,
    execution: tokio::sync::Mutex<()>,
}
impl Service {
    pub fn new(kernel: Kernel, service_uid: u32, provisioning: Provisioning) -> Result<Self> {
        provisioning.validate(service_uid)?;
        Ok(Self {
            kernel,
            provisioning: Some(provisioning),
            executor: None,
            sessions: Mutex::new(BTreeMap::new()),
            observers: Mutex::new(BTreeMap::new()),
            wake: tokio::sync::Notify::new(),
            execution: tokio::sync::Mutex::new(()),
        })
    }
    /// Listen but refuse admission until explicit local mapping is supplied.
    pub fn unprovisioned(kernel: Kernel) -> Self {
        Self {
            kernel,
            provisioning: None,
            executor: None,
            sessions: Mutex::new(BTreeMap::new()),
            observers: Mutex::new(BTreeMap::new()),
            wake: tokio::sync::Notify::new(),
            execution: tokio::sync::Mutex::new(()),
        }
    }
    pub fn with_executor(mut self, executor: Executor) -> Self {
        self.executor = Some(executor);
        self
    }
    /// Authenticated sessions only; downstream Authority still checks every poll.
    pub fn attach(&self, session: Session) -> Result<()> {
        self.attach_token(session, 0)
    }
    fn attach_token(&self, session: Session, token: u64) -> Result<()> {
        let mut sessions = self.sessions.lock().map_err(|_| Error::Denied)?;
        if sessions.len() >= 128 && !sessions.contains_key(&session.peer().to_string()) {
            return Err(Error::Full);
        }
        if sessions
            .get(&session.peer().to_string())
            .is_none_or(|(old, _)| *old <= token)
        {
            sessions.insert(session.peer().to_string(), (token, session));
        }
        self.wake.notify_one();
        Ok(())
    }
    pub fn detach(&self, peer: iroh::EndpointId, token: u64) {
        if let Ok(mut sessions) = self.sessions.lock()
            && sessions
                .get(&peer.to_string())
                .is_some_and(|(old, _)| *old == token)
        {
            sessions.remove(&peer.to_string());
        }
    }
    /// Dispatch readiness follows a successful post-enrollment request. Its
    /// arrival proves the peer received protocol selection; sending before that
    /// reply races independently ordered QUIC streams.
    pub fn connection(
        self: &Arc<Self>,
        session: Session,
        token: u64,
    ) -> Arc<dyn coglet_proto::transport::OperationHandler> {
        Arc::new(Connection {
            service: self.clone(),
            session,
            token,
        })
    }
    /// The caller owns this task and must abort/join it on process shutdown.
    /// Socket waiters never own or cancel admitted execution.
    pub async fn run(self: Arc<Self>) {
        loop {
            if let (Some(p), Some(executor)) = (&self.provisioning, &self.executor)
                && let Ok(pending) = self.kernel.saga_pending(p.uid).await
            {
                for request in pending {
                    let _execution = self.execution.lock().await;
                    let id = request.value()["operation_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let sessions = match self.sessions.lock() {
                        Ok(s) => s.iter().map(|(k, (_, v))| (k.clone(), v.clone())).collect(),
                        Err(_) => return,
                    };
                    let peer = PeerIdentity {
                        uid: p.uid,
                        gid: None,
                        pid: None,
                    };
                    let outcome = self
                        .kernel
                        .saga_pass(peer, request.clone(), p.clone(), executor.clone(), sessions)
                        .await;
                    let outcome = match outcome {
                        Ok(response) => Ok(response),
                        Err(error) => {
                            self.kernel
                                .saga_preflight_failure(p.uid, request, error)
                                .await
                        }
                    };
                    if let Ok(response) = outcome
                        && let Ok(mut observers) = self.observers.lock()
                    {
                        if let Some(sender) = observers.get(&id) {
                            sender.send_replace(response.clone());
                        }
                        if response["receipt"]["state"] != "queued" {
                            observers.remove(&id);
                        }
                    }
                }
            }
            tokio::select! {_=self.wake.notified()=>{},_=tokio::time::sleep(std::time::Duration::from_millis(100))=>{}}
        }
    }
}
struct Connection {
    service: Arc<Service>,
    session: Session,
    token: u64,
}
impl coglet_proto::transport::OperationHandler for Connection {
    fn handle<'a>(
        &'a self,
        context: coglet_proto::transport::RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            let ready = request["method"] != "hub.enroll";
            let reply = self.service.kernel.handle(context, request).await;
            if ready && reply["type"] == "reply" {
                let _ = self.service.attach_token(self.session.clone(), self.token);
            }
            reply
        })
    }
}
pub(crate) fn common_error(error: &Error) -> Value {
    use coglet_proto::authority::AuthorityError;
    let code = match error {
        Error::Invalid | Error::Authority(AuthorityError::InvalidArgument) => "invalid_argument",
        Error::Conflict(_) => "conflict",
        Error::Full | Error::Authority(AuthorityError::ResourceExhausted) => "resource_exhausted",
        Error::Authority(AuthorityError::Unauthenticated) => "unauthenticated",
        _ => "permission_denied",
    };
    json!({"code":code,"message":code,"retryable":false,"details":{}})
}
impl Handler for Service {
    fn admit(
        &self,
        peer: PeerIdentity,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = io::Result<Reply>> + Send + '_>> {
        Box::pin(async move {
            let is_saga = matches!(
                request.value()["op"].as_str(),
                Some("deploy" | "push" | "stop" | "status")
            );
            if is_saga
                && self.executor.is_some()
                && let Some(p) = self.provisioning.as_ref().filter(|p| {
                    peer.uid == p.uid && p.operations.iter().any(|op| request.value()["op"] == *op)
                })
            {
                let id = request.value()["operation_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                // Store admission is already serialized by Kernel::work. Do not
                // hold a socket waiter behind network I/O from an older saga.
                let op = request.value()["op"].clone();
                let response = self.kernel.saga_admit(peer, request, p.clone()).await;
                return match response {
                    Ok(response) if response["receipt"]["state"] == "queued" => {
                        let mut observers = self
                            .observers
                            .lock()
                            .map_err(|_| io::Error::other("observer lock"))?;
                        let sender = observers
                            .entry(id)
                            .or_insert_with(|| tokio::sync::watch::channel(response).0);
                        let receiver = sender.subscribe();
                        self.wake.notify_one();
                        Ok(Reply::Admitted(receiver))
                    }
                    Ok(response) => Ok(Reply::Admitted(tokio::sync::watch::channel(response).1)),
                    Err(error) => Ok(Reply::Rejected(
                        json!({"version":"local-1","operation_id":id,"op":op,"receipt":null,"result":null,"error":common_error(&error)}),
                    )),
                };
            }
            let provisioning = self
                .provisioning
                .as_ref()
                .filter(|p| p.permits(peer, &request));
            let Some(provisioning) = provisioning else {
                return Ok(Reply::Rejected(
                    json!({"version":"local-1","operation_id":request.value()["operation_id"],"op":request.value()["op"],"receipt":null,"result":null,"error":common_error(&Error::Denied)}),
                ));
            };
            let id = request.value()["operation_id"].clone();
            let op = request.value()["op"].clone();
            match self
                .kernel
                .operator_configuration(peer, request, provisioning.clone())
                .await
            {
                Ok(response) => Ok(Reply::Admitted(tokio::sync::watch::channel(response).1)),
                Err(error) => Ok(Reply::Rejected(
                    json!({"version":"local-1","operation_id":id,"op":op,"receipt":null,"result":null,"error":common_error(&error)}),
                )),
            }
        })
    }
}
