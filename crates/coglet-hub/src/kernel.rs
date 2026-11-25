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
#[cfg(unix)]
mod operator;
pub mod promotion;
#[cfg(unix)]
pub mod saga;
#[cfg(test)]
mod tests;

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
struct Liveness {
    since: Duration,
    accepted: Option<Duration>,
    sessions: std::collections::BTreeSet<u64>,
}
struct State {
    elapsed: Arc<dyn Fn() -> Duration + Send + Sync>,
    live: std::collections::BTreeMap<EndpointId, Liveness>,
    next_connection: u64,
    store: Store,
    authority: Authority,
    hub: EndpointId,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
}
#[derive(Default)]
struct DispatchFence {
    active: std::collections::BTreeSet<(String, String)>,
    revoked: std::collections::BTreeSet<EndpointId>,
    ended: std::collections::BTreeSet<(String, String)>,
}
#[derive(Clone)]
pub struct Kernel {
    state: Arc<Mutex<State>>,
    blocking: Arc<Semaphore>,
    control: Arc<Semaphore>,
    dispatch: Arc<Semaphore>,
    boundary: Arc<Mutex<DispatchFence>>,
    changed: tokio::sync::watch::Sender<u64>,
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
        let checked_time = std::cell::Cell::new(None);
        self.authority.authorize_delegation(
            self.hub,
            peer,
            target,
            method,
            || {
                let now = (self.clock)();
                checked_time.set(now);
                now
            },
            generation,
        )?;
        // Delegation verification samples after its blocking lookup/signature
        // work. Re-intersect the grant at THAT sample, not the earlier one.
        // Authority still owns both decisions; this is not a cached lease.
        self.authority.authorize(
            peer,
            method,
            Some(target),
            checked_time.get(),
            r.grant_generation,
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
                                "SELECT request,reply,attempted FROM deliveries WHERE target=?1 AND id=?2",
                                params![target, action_id],
                                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,r.get::<_, i64>(2)?)),
                            )
                            .optional()?
                            .ok_or(Error::Denied)?;
                        let sent: Value = serde_json::from_str(&saved.0)?;
                        let admissible = match saved.1 {
                            Some(reply) => {
                                let reply: Value = serde_json::from_str(&reply)?;
                                reply["result"]["accepted"] == true
                                    || reply["result"]["state"] == "applied"
                                    // A locally retained uncertain observation is
                                    // not a runtime rejection. A matching outcome
                                    // can arrive reliably after receipt lookup failed.
                                    || (saved.2 != 0
                                        && reply["error"]["details"]["unknown_effect"] == true)
                            }
                            // The authenticated runtime can emit its outcome on a
                            // separate stream before the action-accept is observed.
                            None => saved.2 != 0,
                        };
                        if !admissible
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
                let generation: String = self
                    .store
                    .db
                    .query_row(
                        "SELECT generation FROM health_grants WHERE peer=?1",
                        [peer.to_string()],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or(Error::Denied)?;
                self.authority.authorize(
                    peer,
                    method,
                    None,
                    (self.clock)(),
                    generation.parse().map_err(|_| Error::Denied)?,
                )?;
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
                let now = (self.elapsed)();
                if let Some(live) = self.live.get_mut(&peer) {
                    live.accepted = Some(now);
                }
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
        let start = std::time::Instant::now();
        Self::open_with_elapsed(root, hub, clock, Arc::new(move || start.elapsed()))
    }
    /// Trusted monotonic elapsed-clock injection; never use a peer's uptime here.
    pub fn open_with_elapsed(
        root: impl AsRef<Path>,
        hub: EndpointId,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        elapsed: Arc<dyn Fn() -> Duration + Send + Sync>,
    ) -> Result<Self> {
        // Reject any old split entry (including dangling symlinks) before
        // creating a replacement database. Never silently lose enrollment.
        match std::fs::symlink_metadata(root.as_ref().join("authority.db")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(Error::SplitAuthority),
        }
        let store = Store::open(root.as_ref().join("kernel.db"))?;
        store.db.execute_batch("CREATE TABLE IF NOT EXISTS registry(target TEXT PRIMARY KEY, config TEXT NOT NULL); CREATE TABLE IF NOT EXISTS ended_tickets(actor TEXT, ticket TEXT, PRIMARY KEY(actor,ticket)); CREATE TABLE IF NOT EXISTS provisioning(target TEXT PRIMARY KEY, config TEXT NOT NULL); CREATE TABLE IF NOT EXISTS health_grants(peer TEXT PRIMARY KEY, generation TEXT NOT NULL); CREATE TABLE IF NOT EXISTS health(peer TEXT PRIMARY KEY, metadata TEXT NOT NULL); CREATE TABLE IF NOT EXISTS deliveries(target TEXT, id TEXT, intent TEXT NOT NULL, request TEXT NOT NULL, attempted INTEGER NOT NULL DEFAULT 0, reply TEXT, PRIMARY KEY(target,id));")?;
        let authority = Authority::open(root.as_ref().join("kernel.db"))?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                elapsed,
                live: Default::default(),
                next_connection: 0,
                store,
                authority,
                hub,
                clock,
            })),
            blocking: Arc::new(Semaphore::new(8)),
            control: Arc::new(Semaphore::new(2)),
            dispatch: Arc::new(Semaphore::new(8)),
            boundary: Arc::new(Mutex::new(DispatchFence::default())),
            changed: tokio::sync::watch::channel(0).0,
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
        self.configure_invite(peer, grants, move |authority, grants| {
            Ok(authority.issue_invite(peer, ttl, grants)?)
        })
        .await
    }
    /// Trusted local fixture provisioning only, gated out of default builds.
    #[cfg(feature = "test-controls")]
    pub async fn test_invite(
        &self,
        peer: EndpointId,
        secret: String,
        ttl: Duration,
        grants: Vec<Grant>,
    ) -> Result<()> {
        self.configure_invite(peer, grants, move |authority, grants| {
            Ok(authority.test_issue_invite(peer, &secret, ttl, grants)?)
        })
        .await
    }
    /// Close the real enrollment window for a local negative fixture.
    #[cfg(feature = "test-controls")]
    pub async fn test_close_window(&self) -> Result<()> {
        self.work(|s| Ok(s.authority.close_window()?)).await
    }
    async fn configure_invite<T: Send + 'static>(
        &self,
        peer: EndpointId,
        grants: Vec<Grant>,
        issue: impl FnOnce(&Authority, Vec<Grant>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.work(move |s| {
            let health: Vec<_> = grants.iter().filter(|g| g.method=="hub.health" && g.target.is_none()).collect();
            if health.len()>1 {return Err(Error::Invalid);}
            if let Some(g)=health.first() {
                let previous: Option<String> = s.store.db.query_row("SELECT generation FROM health_grants WHERE peer=?1",[peer.to_string()],|r|r.get(0)).optional()?;
                if previous.is_some_and(|p|p!=g.generation.to_string()) { return Err(Error::Conflict(0)); }
            }
            let invite=issue(&s.authority, grants.clone())?;
            if let Some(g)=health.first() {
                let tx=s.store.db.transaction()?;
                tx.execute("INSERT OR IGNORE INTO health_grants VALUES(?1,?2)",params![peer.to_string(),g.generation.to_string()])?;
                audit(&tx,json!({"method":"operator.health_grant","peer":peer.to_string(),"generation":g.generation,"outcome":"configured"}))?;
                tx.commit()?;
            }
            Ok(invite)
        })
            .await
    }
    /// Local transport adapter: token prevents an old connection closing a replacement.
    pub async fn transport_connected(&self, peer: EndpointId) -> Result<u64> {
        self.work(move |s| {
            s.next_connection = s.next_connection.checked_add(1).ok_or(Error::Full)?;
            let id = s.next_connection;
            let now = (s.elapsed)();
            let live = s.live.entry(peer).or_insert_with(|| Liveness {
                since: now,
                accepted: None,
                sessions: Default::default(),
            });
            if live.sessions.is_empty() {
                live.since = now;
                live.accepted = None;
            }
            live.sessions.insert(id);
            Ok(id)
        })
        .await
    }
    pub async fn transport_disconnected(&self, peer: EndpointId, connection: u64) -> Result<()> {
        // Reserved bounded control capacity, not fail-fast bulk admission. Losing
        // cleanup under saturation would keep a dead transport live indefinitely.
        let permit = self
            .control
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Denied)?;
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut s = state.lock().map_err(|_| Error::Denied)?;
            if let Some(live) = s.live.get_mut(&peer) {
                live.sessions.remove(&connection);
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Denied)?
    }
    pub async fn revoke_peer(&self, peer: EndpointId) -> Result<()> {
        let boundary = self.boundary.clone();
        let changed = self.changed.clone();
        self.work(move|s| {
            // Serialize revocation with each synchronous poll of transport dispatch,
            // never with an entire network wait. No request can resume writing after
            // the revocation commit; already sent effects remain explicitly unknown.
            let mut fence = boundary.lock().map_err(|_| Error::Denied)?;
            s.record(json!({"method":"operator.revoke_peer","peer":peer.to_string(),"outcome":"requested"}))?;
            s.authority.revoke_peer(peer)?;
            fence.revoked.insert(peer);
            changed.send_modify(|v| *v = v.wrapping_add(1));
            Ok(())
        }).await
    }
    pub async fn register(&self, r: Registration) -> Result<()> {
        self.work(move|s| {
            if !valid("coglet-id",&json!(r.coglet)) || r.events.len()>256 || r.actions.len()>256 || r.events.iter().any(|g|!valid("surface-id",&json!(g.source))||!valid("capability-name",&json!(g.kind))||g.kind.starts_with("runtime.")) || r.actions.iter().any(|g| !valid("source-id",&json!(g.actor)) || !valid("capability-name",&json!(g.capability)) || g.ticket.as_ref().is_some_and(|t| !valid("correlation-id",&json!(t)))) { return Err(Error::Invalid); }
            let device=r.device.parse().map_err(|_|Error::Invalid)?;
            // Reject malformed/expired initial intent before reserving its identity.
            // Use the shared verifier; Authority remains the sole persistent binding.
            coglet_proto::signing::verify(&r.delegation,"delegation",&r.coglet[7..]).map_err(|_|Error::Denied)?;
            let statement=&r.delegation["statement"];
            if statement["hub"]!=s.hub.to_string() || statement["device"]!=r.device || statement["coglet"]!=r.coglet || !(s.clock)().zip(statement["expires"].as_u64()).is_some_and(|(now,expiry)|now<expiry) {return Err(Error::Denied);}
            // Initial local speech ownership is exclusive, not a subscription.
            // Pending intents reserve it too, so a failed registration cannot
            // race another coglet into widening private owner-unicast visibility.
            let mut q=s.store.db.prepare("SELECT config FROM registry UNION ALL SELECT config FROM provisioning")?;
            for config in q.query_map([],|row|row.get::<_,String>(0))? {
                let other:Registration=serde_json::from_str(&config?)?;
                if other.coglet != r.coglet && other.events.iter().any(|g|g.kind.starts_with("speech.") && r.events.iter().any(|wanted|wanted.kind.starts_with("speech.") && wanted.source==g.source)) {return Err(Error::Denied);}
            }
            drop(q);
            let previous=s.store.db.query_row("SELECT config FROM registry WHERE target=?1",[&r.coglet],|row|row.get::<_,String>(0)).optional()?;
            if let Some(previous)=previous { return if serde_json::from_str::<Registration>(&previous)?==r { Ok(()) } else { Err(Error::Conflict(0)) }; }
            // Cross-database provisioning is a recoverable two-phase local operation.
            // Persist exact intent plus audit BEFORE touching Authority. Only a final
            // registry commit enables use; identical retries repair either crash gap.
            // A pending intent can never be substituted by a different configuration.
            let pending=s.store.db.query_row("SELECT config FROM provisioning WHERE target=?1",[&r.coglet],|row|row.get::<_,String>(0)).optional()?;
            if let Some(pending)=pending {
                if serde_json::from_str::<Registration>(&pending)?!=r {return Err(Error::Conflict(0));}
            } else {
                let tx=s.store.db.transaction()?;
                tx.execute("INSERT INTO provisioning VALUES(?1,?2)",params![r.coglet,serde_json::to_string(&r)?])?;
                audit(&tx,json!({"method":"operator.register","target":r.coglet,"peer":r.device,"outcome":"prepared"}))?;
                tx.commit()?;
            }
            s.authority.accept_delegation(s.hub,device,&r.coglet,&r.delegation,|| (s.clock)())?;
            let tx=s.store.db.transaction()?;
            tx.execute("INSERT INTO registry VALUES(?1,?2)",params![r.coglet,serde_json::to_string(&r)?])?;
            tx.execute("DELETE FROM provisioning WHERE target=?1",[&r.coglet])?;
            audit(&tx,json!({"method":"operator.register","target":r.coglet,"peer":r.device,"outcome":"registered"}))?;
            tx.commit()?; Ok(())
        }).await
    }
    /// Explicit local operator approval to recover a pending registration with a
    /// fresh, strictly higher delegation generation. Policy/device cannot change.
    /// Never call automatically merely because a different signed proof arrived.
    pub async fn recover_registration(&self, r: Registration) -> Result<()> {
        self.work(move |s| {
            let pending: String = s.store.db.query_row("SELECT config FROM provisioning WHERE target=?1",[&r.coglet],|row|row.get(0)).optional()?.ok_or(Error::Denied)?;
            let old: Registration = serde_json::from_str(&pending)?;
            let mut same_policy = r.clone();
            same_policy.delegation = old.delegation.clone();
            if same_policy != old || !r.delegation["statement"]["generation"].as_u64().zip(old.delegation["statement"]["generation"].as_u64()).is_some_and(|(new, old)| new > old) {
                return Err(Error::Denied);
            }
            coglet_proto::signing::verify(&r.delegation,"delegation",&r.coglet[7..]).map_err(|_|Error::Denied)?;
            let statement = &r.delegation["statement"];
            if statement["hub"]!=s.hub.to_string() || statement["device"]!=r.device || statement["coglet"]!=r.coglet || !(s.clock)().zip(statement["expires"].as_u64()).is_some_and(|(now,expiry)|now<expiry) {return Err(Error::Denied);}
            // Separate immutable approval survives a second crash without rewriting
            // the original intent or asking Authority to accept an implicit replace.
            s.store.db.execute_batch("CREATE TABLE IF NOT EXISTS provisioning_recovery(target TEXT PRIMARY KEY, config TEXT NOT NULL)")?;
            let approved: Option<String> = s.store.db.query_row("SELECT config FROM provisioning_recovery WHERE target=?1",[&r.coglet],|row|row.get(0)).optional()?;
            let needs_approval = match approved {
                Some(approved) => {
                    let approved: Registration = serde_json::from_str(&approved)?;
                    if approved == r { false } else {
                        if !r.delegation["statement"]["generation"].as_u64().zip(approved.delegation["statement"]["generation"].as_u64()).is_some_and(|(new, old)| new > old) { return Err(Error::Conflict(0)); }
                        true
                    }
                }
                None => true,
            };
            if needs_approval {
                let tx=s.store.db.transaction()?;
                tx.execute("INSERT INTO provisioning_recovery VALUES(?1,?2) ON CONFLICT(target) DO UPDATE SET config=excluded.config",params![r.coglet,serde_json::to_string(&r)?])?;
                audit(&tx,json!({"method":"operator.recover_registration","target":r.coglet,"peer":r.device,"generation":statement["generation"],"outcome":"approved"}))?;
                tx.commit()?;
            }
            let device=r.device.parse().map_err(|_|Error::Invalid)?;
            // Identical accepted replacement is restart recovery, not another
            // generation increment. Otherwise only strict operator replace can win.
            if s.authority.accept_delegation(s.hub,device,&r.coglet,&r.delegation,|| (s.clock)()).is_err() {
                s.authority.replace_delegation(s.hub,device,&r.coglet,&r.delegation,|| (s.clock)())?;
            }
            let tx=s.store.db.transaction()?;
            tx.execute("INSERT INTO registry VALUES(?1,?2)",params![r.coglet,serde_json::to_string(&r)?])?;
            tx.execute("DELETE FROM provisioning WHERE target=?1",[&r.coglet])?;
            tx.execute("DELETE FROM provisioning_recovery WHERE target=?1",[&r.coglet])?;
            audit(&tx,json!({"method":"operator.recover_registration","target":r.coglet,"peer":r.device,"outcome":"registered"}))?;
            tx.commit()?;
            Ok(())
        }).await
    }
    /// Local trusted runtime only. No remote debug/export authority is implied.
    pub async fn events(&self, target: &str) -> Result<Vec<Value>> {
        let target = target.to_owned();
        self.work(move |s| s.store.pending_events(&target)).await
    }
    /// Trusted local consumer only: stable delivery tokens scoped to this target.
    /// This API must not be exposed as an unauthenticated remote debug endpoint.
    pub async fn event_deliveries(&self, target: &str) -> Result<Vec<(i64, Value)>> {
        let target = target.to_owned();
        self.work(move |s| s.store.event_deliveries(&target)).await
    }
    /// Call only after the consumer durably commits processing through this token.
    pub async fn ack_events(&self, target: &str, through: i64) -> Result<()> {
        let target = target.to_owned();
        self.work(move |s| s.store.ack_events(&target, through))
            .await
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
            let mut devices=std::collections::BTreeMap::<String,Value>::new();
            for r in &coglets {devices.insert(r["device"].as_str().ok_or(Error::Invalid)?.to_owned(),Value::Null);}
            let mut q=s.store.db.prepare("SELECT peer,metadata FROM health ORDER BY peer")?;
            for row in q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                let (peer,text)=row?;devices.insert(peer,serde_json::from_str(&text)?);
            }
            for peer in s.live.keys() {devices.entry(peer.to_string()).or_insert(Value::Null);}
            let now=(s.elapsed)();
            let devices=devices.into_iter().map(|(peer,metadata)| {
                let live=peer.parse::<EndpointId>().ok().and_then(|p|s.live.get(&p));
                let age=live.and_then(|l|l.accepted).map(|t|now.saturating_sub(t).as_millis().min(u64::MAX as u128) as u64);
                let state=match live {
                    Some(l) if !l.sessions.is_empty()=>if now.saturating_sub(l.accepted.unwrap_or(l.since))>Duration::from_secs(30) {"degraded"} else {"connected"},
                    _=>"disconnected",
                };
                json!({"device":peer,"state":state,"last_health_age_ms":age,"health":metadata})
            }).collect::<Vec<_>>();
            Ok(json!({"hub":s.hub.to_string(),"coglets":coglets,"devices":devices}))
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
