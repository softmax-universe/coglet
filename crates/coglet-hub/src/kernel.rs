//! Local operator APIs are deliberately separate from the frozen ALPN methods.
use crate::{Error, Result, Store, audit};
use coglet_proto::{
    authority::{Authority, AuthorityError, Grant},
    transport::{OperationHandler, RequestContext},
};
use iroh::EndpointId;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
mod actions;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventGrant {
    pub source: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionGrant {
    pub actor: String,
    pub ticket: Option<String>,
    pub capability: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub coglet: String,
    pub device: String,
    pub grant_generation: u64,
    pub delegation: Value,
    pub events: Vec<EventGrant>,
    pub actions: Vec<ActionGrant>,
}
struct State {
    store: Store,
    authority: Authority,
    hub: EndpointId,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
}
#[derive(Clone)]
pub struct Kernel {
    state: Arc<Mutex<State>>,
    blocking: Arc<Semaphore>,
    boundary: Arc<tokio::sync::Mutex<()>>,
}
fn valid(rule: &str, value: &Value) -> bool {
    coglet_proto::wire::encode(value).is_ok_and(|bytes| {
        coglet_types::decode_profile(&bytes).is_ok_and(|v| coglet_types::validate(rule, &v).is_ok())
    })
}
impl State {
    fn record(&mut self, record: Value) -> Result<()> {
        let tx = self.store.db.transaction()?;
        audit(&tx, record)?;
        tx.commit()?;
        Ok(())
    }
    fn registration(&self, target: &str) -> Result<Registration> {
        let text = self
            .store
            .db
            .query_row(
                "SELECT config FROM registry WHERE target=?1",
                [target],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .ok_or(Error::Denied)?;
        Ok(serde_json::from_str(&text)?)
    }
    fn authorize(&self, peer: EndpointId, target: &str, method: &str) -> Result<Registration> {
        let r = self.registration(target)?;
        if r.device != peer.to_string() {
            return Err(Error::Denied);
        }
        self.authority.authorize(
            peer,
            method,
            Some(target),
            (self.clock)(),
            r.grant_generation,
        )?;
        let generation = r.delegation["statement"]["generation"]
            .as_u64()
            .ok_or(Error::Denied)?;
        self.authority.authorize_delegation(
            self.hub,
            peer,
            target,
            method,
            || (self.clock)(),
            generation,
        )?;
        Ok(r)
    }
    fn operation(&mut self, context: RequestContext, request: &Value) -> Result<Value> {
        if context.local != self.hub || !valid("hub-request", request) {
            return Err(Error::Invalid);
        }
        if context.deadline <= tokio::time::Instant::now() {
            return Err(Error::Full);
        }
        let method = request["method"].as_str().ok_or(Error::Invalid)?;
        let peer = context.peer;
        match method {
            "hub.enroll" => {
                // A pre-commit attempt record makes an interrupted enrollment recoverable
                // without ever storing the invite, including when the final audit fails.
                self.record(json!({"method":method,"peer":peer.to_string(),"operation_id":request["correlation_id"],"outcome":"attempt"}))?;
                self.authority
                    .enroll(peer, request["params"]["invite"].as_str())?;
                self.record(json!({"method":method,"peer":peer.to_string(),"operation_id":request["correlation_id"],"outcome":"enrolled"}))?;
                Ok(
                    json!({"selection":{"version":"0.1"},"device":peer.to_string(),"hub":self.hub.to_string()}),
                )
            }
            "hub.events" => {
                let target = request["target"].as_str().ok_or(Error::Invalid)?;
                let r = self.authorize(peer, target, method)?;
                let events = request["params"]["events"]
                    .as_array()
                    .ok_or(Error::Invalid)?;
                for e in events {
                    let source = e["source"].as_str().ok_or(Error::Invalid)?;
                    let kind = e["kind"].as_str().ok_or(Error::Invalid)?;
                    // Coglet action outcomes require retained action identity, added by the
                    // dispatch path. Everything else must be an explicitly owned surface.
                    if source == target && kind == "action.outcome" {
                        if !valid("action-outcome", &e["payload"]) {
                            return Err(Error::Denied);
                        }
                        let action_id = e["payload"]["id"].as_str().ok_or(Error::Denied)?;
                        let saved = self
                            .store
                            .db
                            .query_row(
                                "SELECT request,reply FROM deliveries WHERE target=?1 AND id=?2",
                                params![target, action_id],
                                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                            )
                            .optional()?
                            .ok_or(Error::Denied)?;
                        let sent: Value = serde_json::from_str(&saved.0)?;
                        let reply: Value = serde_json::from_str(&saved.1.ok_or(Error::Denied)?)?;
                        if reply["result"]["accepted"] != true
                            || e["correlation_id"] != sent["params"]["action"]["correlation_id"]
                            || e["payload"]["correlation_id"] != e["correlation_id"]
                        {
                            return Err(Error::Denied);
                        }
                    } else if !source.starts_with("surface:")
                        || kind.starts_with("runtime.")
                        || !r
                            .events
                            .iter()
                            .any(|g| g.source == source && g.kind == kind)
                    {
                        return Err(Error::Denied);
                    }
                }
                let offset = request["params"]["offset"].as_u64().ok_or(Error::Invalid)?;
                self.authorize(peer, target, method)?;
                let next = self
                    .store
                    .admit_events(&peer.to_string(), target, offset, events)?;
                Ok(json!({"next_offset":next}))
            }
            "hub.health" => {
                self.authority
                    .authorize(peer, method, None, (self.clock)(), 0)?;
                if request["params"]["device"] != peer.to_string() {
                    return Err(Error::Denied);
                }
                for item in request["params"]["coglets"]
                    .as_array()
                    .ok_or(Error::Invalid)?
                {
                    let r = self.registration(item["coglet"].as_str().ok_or(Error::Invalid)?)?;
                    if r.device != peer.to_string() {
                        return Err(Error::Denied);
                    }
                }
                self.store.db.execute("INSERT INTO health(peer,metadata) VALUES(?1,?2) ON CONFLICT(peer) DO UPDATE SET metadata=excluded.metadata",params![peer.to_string(),serde_json::to_string(&request["params"])?])?;
                Ok(json!({}))
            }
            _ => Err(Error::Denied),
        }
    }
}
impl Kernel {
    pub fn open(
        root: impl AsRef<Path>,
        hub: EndpointId,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ) -> Result<Self> {
        let store = Store::open(root.as_ref().join("kernel.db"))?;
        store.db.execute_batch("CREATE TABLE IF NOT EXISTS registry(target TEXT PRIMARY KEY, config TEXT NOT NULL); CREATE TABLE IF NOT EXISTS health(peer TEXT PRIMARY KEY, metadata TEXT NOT NULL); CREATE TABLE IF NOT EXISTS deliveries(target TEXT, id TEXT, intent TEXT NOT NULL, request TEXT NOT NULL, attempted INTEGER NOT NULL DEFAULT 0, reply TEXT, PRIMARY KEY(target,id));")?;
        let authority = Authority::open(root.as_ref().join("authority.db"))?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                store,
                authority,
                hub,
                clock,
            })),
            blocking: Arc::new(Semaphore::new(8)),
            boundary: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    async fn work<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut State) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .blocking
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Full)?;
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut state = state.lock().map_err(|_| Error::Denied)?;
            f(&mut state)
        })
        .await
        .map_err(|_| Error::Denied)?
    }
    pub async fn open_window(&self, duration: Duration) -> Result<()> {
        self.work(move |s| Ok(s.authority.open_window(duration)?))
            .await
    }
    pub async fn invite(
        &self,
        peer: EndpointId,
        ttl: Duration,
        grants: Vec<Grant>,
    ) -> Result<String> {
        self.work(move |s| Ok(s.authority.issue_invite(peer, ttl, grants)?))
            .await
    }
    pub async fn revoke_peer(&self, peer: EndpointId) -> Result<()> {
        let _boundary = self.boundary.lock().await;
        self.work(move|s| { s.record(json!({"method":"operator.revoke_peer","peer":peer.to_string(),"outcome":"requested"}))?; s.authority.revoke_peer(peer)?; Ok(()) }).await
    }
    pub async fn register(&self, r: Registration) -> Result<()> {
        self.work(move|s| {
            if !valid("coglet-id",&json!(r.coglet)) || r.events.len()>256 || r.actions.len()>256 || r.events.iter().any(|g|!valid("surface-id",&json!(g.source))||!valid("capability-name",&json!(g.kind))||g.kind.starts_with("runtime.")) || r.actions.iter().any(|g| !valid("source-id",&json!(g.actor)) || !valid("capability-name",&json!(g.capability)) || g.ticket.as_ref().is_some_and(|t| !valid("correlation-id",&json!(t)))) { return Err(Error::Invalid); }
            let device=r.device.parse().map_err(|_|Error::Invalid)?;
            s.authority.accept_delegation(s.hub,device,&r.coglet,&r.delegation,|| (s.clock)())?;
            let previous=s.store.db.query_row("SELECT config FROM registry WHERE target=?1",[&r.coglet],|row|row.get::<_,String>(0)).optional()?;
            if let Some(previous)=previous { return if serde_json::from_str::<Registration>(&previous)?==r { Ok(()) } else { Err(Error::Conflict(0)) }; }
            let tx=s.store.db.transaction()?;
            tx.execute("INSERT INTO registry VALUES(?1,?2)",params![r.coglet,serde_json::to_string(&r)?])?;
            audit(&tx,json!({"method":"operator.register","target":r.coglet,"peer":r.device,"outcome":"registered"}))?;
            tx.commit()?; Ok(())
        }).await
    }
    /// Local trusted runtime only. No remote debug/export authority is implied.
    pub async fn events(&self, target: &str) -> Result<Vec<Value>> {
        let target = target.to_owned();
        self.work(move |s| s.store.pending_events(&target)).await
    }
    pub async fn audit(&self) -> Result<Vec<Value>> {
        self.work(|s| s.store.audit()).await
    }
    /// Explicit local operator snapshot: metadata only, no event payloads or prompts.
    pub async fn status(&self) -> Result<Value> {
        self.work(|s| {
            let mut query=s.store.db.prepare("SELECT config FROM registry ORDER BY target")?;
            let configs=query.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let coglets=configs.into_iter().map(|r| { let r:Registration=serde_json::from_str(&r)?; Ok(json!({"coglet":r.coglet,"device":r.device,"events":r.events,"actions":r.actions})) }).collect::<Result<Vec<_>>>()?;
            Ok(json!({"hub":s.hub.to_string(),"coglets":coglets}))
        }).await
    }
}
impl OperationHandler for Kernel {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            let id = request["correlation_id"].clone();
            match self.work(move |s| s.operation(context, &request)).await {
                Ok(result) => json!({"type":"reply","correlation_id":id,"result":result}),
                Err(error) => failure(id, error),
            }
        })
    }
}
fn failure(id: Value, error: Error) -> Value {
    let code = match &error {
        Error::Conflict(_) => "conflict",
        Error::Invalid => "invalid_argument",
        Error::Full => "resource_exhausted",
        Error::Denied => "permission_denied",
        Error::Authority(e) => match e {
            AuthorityError::Unauthenticated => "unauthenticated",
            AuthorityError::PermissionDenied => "permission_denied",
            AuthorityError::ResourceExhausted => "resource_exhausted",
            AuthorityError::InvalidArgument => "invalid_argument",
            AuthorityError::Storage => "unavailable",
        },
        _ => "unavailable",
    };
    let details = if let Error::Conflict(expected) = error {
        json!({"expected_offset":expected})
    } else {
        json!({})
    };
    json!({"type":"error","correlation_id":id,"error":{"code":code,"message":code,"retryable":matches!(code,"unavailable"|"resource_exhausted"),"details":details}})
}
