//! Durable local initial-deploy coordinator. Runtime effects are observed only
//! through the authenticated peer, never through local fixture receipts.
use super::*;
use crate::{
    build,
    operator::{PeerIdentity, Request, service::Provisioning},
};
use coglet_proto::{transport::Session, validator::Validator};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

/// Protected startup configuration, not operator request authority. This finite
/// profile supports initial installation only, not installed-target activation.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub target: build::Target,
    pub profile: build::Profile,
    pub delegation: build::Delegation,
    pub generation: u64,
    pub validator: PathBuf,
    pub quarantine: PathBuf,
}
#[derive(Clone)]
pub struct Executor {
    pub key: iroh::SecretKey,
    pub recipes: Vec<Recipe>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Progress {
    actor: String,
    children: Vec<Value>,
    outcomes: Vec<Option<Value>>,
    attempted: Vec<bool>,
}
fn child(uid: u32, id: &str, method: &str) -> String {
    let bytes = Sha256::digest(format!("local-1/saga/{uid}/{id}/{method}"));
    hex::encode(&bytes[..16])
}
fn envelope(id: String, actor: &str, target: &Value, method: &str, params: Value) -> Value {
    // Absent ticket is omitted, never null.
    json!({"correlation_id":id,"method":method,"target":target,
        "timeout_ms":10000,"context":{"actor":actor},"params":params})
}
fn permitted(p: &Provisioning, peer: PeerIdentity, request: &Request) -> bool {
    peer.uid == p.uid && p.operations.iter().any(|op| request.value()["op"] == *op)
}
fn authorize(
    s: &State,
    peer: EndpointId,
    target: &str,
    method: &str,
    p: &Provisioning,
) -> Result<()> {
    let enrolled: bool = s.store.db.query_row(
        "SELECT EXISTS(SELECT 1 FROM proto_peers WHERE peer=?1)",
        [peer.to_string()],
        |r| r.get(0),
    )?;
    if !enrolled {
        return Err(Error::Authority(AuthorityError::Unauthenticated));
    }
    let r = s.authorize(peer, target, method)?;
    if !p.grants.iter().any(|g| {
        g.peer == r.device
            && g.method == method
            && g.target.as_deref() == Some(target)
            && (s.clock)().is_some_and(|now| now < g.max_expires_unix_s)
    }) {
        return Err(Error::Denied);
    }
    Ok(())
}
fn load(s: &State, uid: u32, id: &str) -> Result<Option<Progress>> {
    let text: Option<String> = s
        .store
        .db
        .query_row(
            "SELECT progress FROM operator_sagas WHERE uid=?1 AND id=?2",
            params![uid, id],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|v| serde_json::from_str(&v).map_err(Error::from))
        .transpose()
}
fn save(s: &mut State, uid: u32, id: &str, progress: &Progress) -> Result<()> {
    let tx = s.store.db.transaction()?;
    tx.execute("INSERT INTO operator_sagas VALUES(?1,?2,?3) ON CONFLICT(uid,id) DO UPDATE SET progress=excluded.progress",
        params![uid,id,serde_json::to_string(progress)?])?;
    audit(
        &tx,
        json!({"method":"operator.saga.progress","uid":uid,"operation_id":id,
        "children":progress.children,"outcomes":progress.outcomes,"attempted":progress.attempted}),
    )?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn initialize(db: &rusqlite::Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS operator_sagas(uid INTEGER NOT NULL,id TEXT NOT NULL,progress TEXT NOT NULL,PRIMARY KEY(uid,id));")?;
    Ok(())
}
impl Kernel {
    pub(crate) async fn saga_admit(
        &self,
        peer: PeerIdentity,
        request: Request,
        p: Provisioning,
    ) -> Result<Value> {
        self.work(move |s| {
            if !permitted(&p, peer, &request) {
                return Err(Error::Denied);
            }
            s.store.operator_admit_actor(peer, &request, Some(&p.actor))
        })
        .await
    }
    pub(crate) async fn saga_preflight_failure(
        &self,
        uid: u32,
        request: Request,
        error: Error,
    ) -> Result<Value> {
        self.work(move |s| {
            let id = request.value()["operation_id"]
                .as_str()
                .ok_or(Error::Invalid)?;
            // Storage/unknown downstream errors are never terminalized here.
            if matches!(
                error,
                Error::Storage(_) | Error::Authority(AuthorityError::Storage) | Error::Full
            ) || load(s, uid, id)?.is_some_and(|p| p.attempted.iter().any(|v| *v))
            {
                let text: String = s.store.db.query_row(
                    "SELECT response FROM operator_operations WHERE uid=?1 AND id=?2",
                    params![uid, id],
                    |r| r.get(0),
                )?;
                return Ok(serde_json::from_str(&text)?);
            }
            s.store.operator_finish(
                uid,
                &request,
                Some(crate::operator::service::common_error(&error)),
                |_| Ok(Value::Null),
            )
        })
        .await
    }
    /// Pending intents retain the original timeout-independent request. Recovery
    /// requires the same protected local actor mapping; it never adopts new scope.
    pub async fn saga_pending(&self, uid: u32) -> Result<Vec<Request>> {
        self.work(move |s| {
            let mut q=s.store.db.prepare("SELECT intent FROM operator_operations WHERE uid=?1 AND json_extract(response,'$.receipt.state')='queued'")?;
            let rows=q.query_map([uid],|r|r.get::<_,Vec<u8>>(0))?;
            let mut pending=Vec::new();
            for row in rows {
                let mut v=coglet_proto::wire::decode(&row?).map_err(|_|Error::Invalid)?;
                if matches!(v["op"].as_str(),Some("deploy"|"push"|"stop"|"status")) {
                    v["timeout_ms"]=json!(1);
                    pending.push(Request::parse(&serde_json::to_vec(&v)?)?);
                }
            }
            Ok(pending)
        }).await
    }
    // Build/publication uses a separate connection to the SAME Store file, with
    // the existing builder's atomic artifact/audit custody. Never hold State
    // across compiler, Git, or validator subprocesses. Dispatch is fenced again.
    async fn saga_build(
        &self,
        peer: PeerIdentity,
        request: Request,
        p: Provisioning,
        executor: Executor,
    ) -> Result<()> {
        let r = request.clone();
        let exec = executor.clone();
        let prepared = self
            .work(move |s| {
                if !permitted(&p, peer, &r) || exec.key.public() != s.hub {
                    return Err(Error::Denied);
                }
                let response = s.store.operator_admit_actor(peer, &r, Some(&p.actor))?;
                let v = r.value();
                let id = v["operation_id"].as_str().ok_or(Error::Invalid)?;
                if response["receipt"]["state"] != "queued"
                    || !matches!(v["op"].as_str(), Some("deploy" | "push"))
                    || load(s, peer.uid, id)?.is_some_and(|p| !p.children.is_empty())
                    || s.store.build_artifact(peer.uid, id)?.is_some()
                {
                    return Ok(None);
                }
                let recipe = exec
                    .recipes
                    .iter()
                    .find(|x| {
                        x.target.coglet == v["params"]["target"]
                            && v["params"]["device"] == x.target.device
                            && v["params"]["delegation_id"] == x.delegation.id
                    })
                    .ok_or(Error::Denied)?
                    .clone();
                let device = recipe.target.device.parse().map_err(|_| Error::Invalid)?;
                for method in if v["op"] == "deploy" {
                    &[
                        "hub.git.push",
                        "hub.receipt",
                        "hub.status",
                        "hub.install",
                        "hub.start",
                    ][..]
                } else {
                    &["hub.git.push", "hub.receipt", "hub.status"][..]
                } {
                    authorize(s, device, &recipe.target.coglet, method, &p)?;
                }
                let path = PathBuf::from(s.store.db.path().ok_or(Error::Invalid)?);
                Ok(Some((recipe, path, s.authority.clone(), s.clock.clone())))
            })
            .await?;
        if let Some((recipe, path, authority, clock)) = prepared {
            let permit = self
                .blocking
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::Full)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let v = request.value();
                let mut store = Store::open(path)?;
                build::Builder::new(recipe.target, recipe.profile)?
                    .compile(&build::BuildRequest {
                        source_commit: v["params"]["source_commit"]
                            .as_str()
                            .ok_or(Error::Invalid)?
                            .into(),
                        profile_id: v["params"]["build_profile"]
                            .as_str()
                            .ok_or(Error::Invalid)?
                            .into(),
                    })?
                    .sign_and_retain(
                        &mut store,
                        &build::SignRequest {
                            uid: peer.uid,
                            operation_id: v["operation_id"].as_str().ok_or(Error::Invalid)?.into(),
                            delegation_id: recipe.delegation.id.clone(),
                            old_tip: None,
                            generation: recipe.generation,
                        },
                        &recipe.delegation,
                        &executor.key,
                        &authority,
                        &Validator::new(&recipe.validator),
                        &recipe.quarantine,
                        || clock(),
                    )?;
                Ok::<_, Error>(())
            })
            .await
            .map_err(|_| Error::Denied)??;
        }
        Ok(())
    }
    /// One bounded recovery pass. A queued or uncertain child never authorizes
    /// the next stage; future passes poll its ORIGINAL ID and original context.
    pub async fn saga_pass(
        &self,
        peer: PeerIdentity,
        request: Request,
        p: Provisioning,
        executor: Executor,
        sessions: BTreeMap<String, Session>,
    ) -> Result<Value> {
        self.saga_build(peer, request.clone(), p.clone(), executor.clone())
            .await?;
        let request = Arc::new(request);
        let id = request.value()["operation_id"]
            .as_str()
            .ok_or(Error::Invalid)?
            .to_owned();
        let r = request.clone();
        let provision = p.clone();
        let exec = executor.clone();
        let key = id.clone();
        let (response, progress)=self.work(move |s| {
            if !permitted(&provision,peer,&r) || exec.key.public()!=s.hub { return Err(Error::Denied); }
            let response=s.store.operator_admit_actor(peer,&r,Some(&provision.actor))?;
            if response["receipt"]["state"]!="queued" { return Ok((response,None)); }
            s.store.db.execute_batch("CREATE TABLE IF NOT EXISTS operator_sagas(uid INTEGER NOT NULL,id TEXT NOT NULL,progress TEXT NOT NULL,PRIMARY KEY(uid,id));")?;
            if let Some(old)=load(s,peer.uid,&key)? {
                if old.actor!=provision.actor { return Err(Error::Denied); }
                if !old.children.is_empty() { return Ok((response,Some(old))); }
            } else {
                save(s,peer.uid,&key,&Progress {actor:provision.actor.clone(),children:vec![],outcomes:vec![],attempted:vec![]})?;
            }
            let v=r.value(); let target=&v["params"]["target"];
            let device=v["params"]["device"].as_str().ok_or(Error::Invalid)?.parse().map_err(|_|Error::Invalid)?;
            let op=v["op"].as_str().ok_or(Error::Invalid)?;
            let mut children=Vec::new();
            if matches!(op,"deploy"|"push") {
                let recipe=exec.recipes.iter().find(|x|x.target.coglet==*target && v["params"]["device"]==x.target.device
                    && v["params"]["delegation_id"]==x.delegation.id).ok_or(Error::Denied)?;
                for method in if op=="deploy" {&["hub.git.push","hub.receipt","hub.status","hub.install","hub.start"][..]} else {&["hub.git.push","hub.receipt","hub.status"][..]} {
                    authorize(s,device,target.as_str().ok_or(Error::Invalid)?,method,&provision)?;
                }
                // Retained artifacts are retrieved without rebuilding after a
                // crash; protected recipe/source changes cannot replace them.
                let artifact=s.store.build_artifact(peer.uid,&key)?.ok_or(Error::Invalid)?;
                children.push(envelope(child(peer.uid,&key,"preflight"),&provision.actor,target,"hub.status",json!({})));
                children.push(envelope(child(peer.uid,&key,"push"),&provision.actor,target,"hub.git.push",
                    json!({"update":artifact.signed,"size":artifact.bundle.len(),"sha256":hex::encode(Sha256::digest(&artifact.bundle))})));
                if op=="deploy" {
                    children.push(envelope(child(peer.uid,&key,"install"),&provision.actor,target,"hub.install",
                        json!({"initial":artifact.signed,"delegation":recipe.delegation.signed})));
                    children.push(envelope(child(peer.uid,&key,"start"),&provision.actor,target,"hub.start",json!({})));
                    children.push(envelope(child(peer.uid,&key,"running"),&provision.actor,target,"hub.status",json!({})));
                }
            } else {
                authorize(s,device,target.as_str().ok_or(Error::Invalid)?,if op=="stop" {"hub.stop"} else {"hub.status"},&provision)?;
                if op=="stop" {authorize(s,device,target.as_str().ok_or(Error::Invalid)?,"hub.receipt",&provision)?;}
                children.push(envelope(child(peer.uid,&key,op),&provision.actor,target,
                    if op=="stop" {"hub.stop"} else {"hub.status"},json!({})));
            }
            let progress=Progress {actor:provision.actor.clone(),outcomes:vec![None;children.len()],attempted:vec![false;children.len()],children};
            save(s,peer.uid,&key,&progress)?;
            Ok((response,Some(progress)))
        }).await?;
        let Some(mut progress) = progress else {
            return Ok(response);
        };
        let device = request.value()["params"]["device"]
            .as_str()
            .ok_or(Error::Invalid)?;
        let Some(session) = sessions.get(device) else {
            return Ok(response);
        };
        for i in 0..progress.children.len() {
            if progress.outcomes[i]
                .as_ref()
                .is_some_and(|o| o["state"] == "applied")
            {
                continue;
            }
            let original = progress.children[i].clone();
            let status = original["method"] == "hub.status";
            let mut reply = None;
            if progress.attempted[i] && !status {
                let receipt = envelope(
                    child(peer.uid, &id, &format!("poll-{i}")),
                    &progress.actor,
                    &original["target"],
                    "hub.receipt",
                    json!({"operation_id":original["correlation_id"]}),
                );
                match self
                    .saga_send(session, receipt, original.clone(), p.clone(), None)
                    .await
                {
                    Ok(v) if v["type"] == "reply" => reply = Some(v),
                    Ok(v) if v["error"]["code"] == "not_found" => {}
                    _ => return Ok(response),
                }
            }
            if reply.is_none() {
                // Persist attempted BEFORE polling transport, including before
                // any credit is issued. A crash thereafter reconciles first.
                progress.attempted[i] = true;
                let snapshot = progress.clone();
                let key = id.clone();
                self.work(move |s| save(s, peer.uid, &key, &snapshot))
                    .await?;
                let bundle = if original["method"] == "hub.git.push" {
                    let key = id.clone();
                    Some(
                        self.work(move |s| {
                            Ok(s.store
                                .build_artifact(peer.uid, &key)?
                                .ok_or(Error::Invalid)?
                                .bundle)
                        })
                        .await?,
                    )
                } else {
                    None
                };
                match self
                    .saga_send(
                        session,
                        original.clone(),
                        original.clone(),
                        p.clone(),
                        bundle,
                    )
                    .await
                {
                    Ok(v) => reply = Some(v),
                    Err(_) => return Ok(response),
                }
            }
            let reply = reply.ok_or(Error::Invalid)?;
            if reply["type"] == "error" {
                if reply["error"]["retryable"] == true {
                    // A transient refusal is not an effect receipt. Retain
                    // attempted custody; the next pass reconciles the same ID.
                    return Ok(response);
                }
                // A peer refusal is authoritative; never hide partial earlier
                // success, whose receipts remain in the durable progress/audit.
                let error = reply["error"].clone();
                progress.outcomes[i] = Some(reply);
                return self
                    .saga_terminal(peer.uid, request, progress, "failed", Some(error))
                    .await;
            }
            let result = reply["result"].clone();
            if status
                && request.value()["op"] == "deploy"
                && if i == 0 {
                    !result["live_commit"].is_null()
                        && result["live_commit"]
                            != progress.children[1]["params"]["update"]["statement"]["new"]
                } else {
                    result["state"] != "running"
                        || result["live_commit"]
                            != progress.children[1]["params"]["update"]["statement"]["new"]
                }
            {
                progress.outcomes[i] = Some(json!({"observation":result}));
                return self
                    .saga_terminal(
                        peer.uid,
                        request,
                        progress,
                        "failed",
                        Some(crate::operator::service::common_error(&Error::Conflict(0))),
                    )
                    .await;
            }
            if !status && result["operation_id"] != original["correlation_id"] {
                return Ok(response);
            }
            if !status && result["state"] != "applied" {
                if matches!(result["state"].as_str(), Some("failed" | "cancelled")) {
                    let cancelled = result["state"] == "cancelled";
                    let error = (!cancelled).then(|| result["error"].clone());
                    progress.outcomes[i] = Some(result);
                    return self
                        .saga_terminal(
                            peer.uid,
                            request,
                            progress,
                            if cancelled { "cancelled" } else { "failed" },
                            error,
                        )
                        .await;
                }
                progress.outcomes[i] = Some(result);
                let snapshot = progress.clone();
                let key = id.clone();
                self.work(move |s| save(s, peer.uid, &key, &snapshot))
                    .await?;
                return Ok(response);
            }
            progress.outcomes[i] = Some(if status {
                json!({"state":"applied","observation":result})
            } else {
                result
            });
            let snapshot = progress.clone();
            let key = id.clone();
            self.work(move |s| save(s, peer.uid, &key, &snapshot))
                .await?;
        }
        let result = if request.value()["op"] == "status" {
            progress.outcomes[0].as_ref().ok_or(Error::Invalid)?["observation"].clone()
        } else {
            Value::Null
        };
        self.work(move |s| {
            s.store
                .operator_finish(peer.uid, &request, None, |_| Ok(result))
        })
        .await
    }
    async fn saga_terminal(
        &self,
        uid: u32,
        request: Arc<Request>,
        progress: Progress,
        state: &'static str,
        error: Option<Value>,
    ) -> Result<Value> {
        self.work(move |s| {
            s.store.operator_terminal(uid,&request,state,error,|tx| {
                let id=request.value()["operation_id"].as_str().ok_or(Error::Invalid)?;
                tx.execute("UPDATE operator_sagas SET progress=?1 WHERE uid=?2 AND id=?3",params![serde_json::to_string(&progress)?,uid,id])?;
                audit(tx,json!({"method":"operator.saga.terminal","uid":uid,"operation_id":id,"children":progress.children,"outcomes":progress.outcomes}))?;
                Ok(Value::Null)
            })
        }).await
    }
    async fn saga_send(
        &self,
        session: &Session,
        request: Value,
        original: Value,
        p: Provisioning,
        bundle: Option<Vec<u8>>,
    ) -> Result<Value> {
        let permit = self
            .dispatch
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Full)?;
        struct Cancel(tokio::sync::watch::Sender<bool>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.send_replace(true);
            }
        }
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let _cancel = Cancel(cancel);
        let state = self.state.clone();
        let boundary = self.boundary.clone();
        let mut changed = self.changed.subscribe();
        let session = session.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            runtime.block_on(async move {
                let peer = session.peer();
                let target = request["target"].as_str().ok_or(Error::Invalid)?.to_owned();
                let method = request["method"].as_str().ok_or(Error::Invalid)?.to_owned();
                let mut source = std::io::Cursor::new(bundle.unwrap_or_default());
                let mut sending: Pin<
                    Box<
                        dyn Future<
                                Output = std::result::Result<
                                    Value,
                                    coglet_proto::transport::TransportError,
                                >,
                            > + Send,
                    >,
                > = if method == "hub.git.push" {
                    Box::pin(session.push_git(request, &mut source))
                } else {
                    Box::pin(session.request(request))
                };
                let cancel_view = cancelled.clone();
                loop {
                    let guarded = std::future::poll_fn(|cx| {
                        let s = state.lock().map_err(|_| Error::Denied)?;
                        let fence = boundary.lock().map_err(|_| Error::Denied)?;
                        if *cancel_view.borrow() || fence.revoked.contains(&peer) {
                            return std::task::Poll::Ready(Err(Error::Denied));
                        }
                        authorize(&s, peer, &target, &method, &p)?;
                        if method == "hub.receipt" {
                            authorize(
                                &s,
                                peer,
                                &target,
                                original["method"].as_str().ok_or(Error::Invalid)?,
                                &p,
                            )?;
                        }
                        sending
                            .as_mut()
                            .poll(cx)
                            .map(|v| v.map_err(|_| Error::Denied))
                    });
                    tokio::select! { biased;
                        _=cancelled.changed()=>return Err(Error::Denied),
                        _=changed.changed()=>continue,
                        result=guarded=>return result,
                        _=tokio::time::sleep(Duration::from_millis(25))=>continue,
                    }
                }
            })
        })
        .await
        .map_err(|_| Error::Denied)?
    }
}
