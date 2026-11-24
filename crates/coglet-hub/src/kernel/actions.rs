use super::*;
use coglet_proto::transport::Session;

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod receipt_tests;

type RuntimeClock = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

fn check_action_deadline(request: &Value, clock: Option<&RuntimeClock>) -> Result<()> {
    let now = clock.and_then(|clock| clock()).ok_or(Error::Denied)?;
    let deadline = request["params"]["action"]["deadline"]
        .as_u64()
        .ok_or(Error::Invalid)?;
    if now >= deadline {
        return Err(Error::Denied);
    }
    Ok(())
}

struct ActiveDispatch {
    boundary: Arc<Mutex<DispatchFence>>,
    key: (String, String),
}
impl Drop for ActiveDispatch {
    fn drop(&mut self) {
        if let Ok(mut fence) = self.boundary.lock() {
            fence.active.remove(&self.key);
        }
    }
}

impl State {
    fn action_authority(&self, target: &str, request: &Value) -> Result<Registration> {
        let r = self.registration(target)?;
        let peer = r.device.parse().map_err(|_| Error::Denied)?;
        self.authorize(peer, target, "hub.actions")?;
        let c = &request["context"];
        if let (Some(actor), Some(ticket)) = (c["actor"].as_str(), c["ticket"].as_str()) {
            let ended: bool = self.store.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM ended_tickets WHERE actor=?1 AND ticket=?2)",
                params![actor, ticket],
                |r| r.get(0),
            )?;
            if ended {
                return Err(Error::Denied);
            }
        }
        if !r.actions.iter().any(|g| {
            Some(g.actor.as_str()) == c["actor"].as_str()
                && g.ticket.as_deref() == c["ticket"].as_str()
                && g.capability == request["params"]["action"]["capability"]
        }) {
            return Err(Error::Denied);
        }
        Ok(r)
    }
}
impl Kernel {
    /// Trusted local operator only. A ticket tombstone is permanent for this identity.
    /// Pending sends are fenced immediately; sent actions cannot be physically undone.
    pub async fn end_ticket(&self, actor: &str, ticket: &str) -> Result<()> {
        if !valid("source-id", &json!(actor)) || !valid("correlation-id", &json!(ticket)) {
            return Err(Error::Invalid);
        }
        let actor = actor.to_owned();
        let ticket = ticket.to_owned();
        let boundary = self.boundary.clone();
        let changed = self.changed.clone();
        self.work(move |s| {
            let mut fence=boundary.lock().map_err(|_| Error::Denied)?;
            let tx=s.store.db.transaction()?;
            tx.execute("INSERT OR IGNORE INTO ended_tickets VALUES(?1,?2)",params![actor,ticket])?;
            let pending={
                let mut q=tx.prepare("SELECT target,id,request,attempted FROM deliveries WHERE reply IS NULL")?;
                q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?
            };
            for (target,id,text,attempted) in pending {
                let request:Value=serde_json::from_str(&text)?;
                if request["context"]["actor"]!=actor || request["context"]["ticket"]!=ticket {continue;}
                let mut reply=failure(json!(id),Error::Denied);
                reply["error"]["details"]=json!({"unknown_effect":attempted!=0,"ticket_ended":true});
                tx.execute("UPDATE deliveries SET reply=?3 WHERE target=?1 AND id=?2",params![target,id,serde_json::to_string(&reply)?])?;
                audit(&tx,json!({"method":"hub.actions.cancel","operation_id":id,"target":target,"actor":actor,"ticket":ticket,"outcome":if attempted==0 {"cancelled"} else {"unknown_effect"}}))?;
            }
            audit(&tx,json!({"method":"operator.end_ticket","actor":actor,"ticket":ticket,"outcome":"ended"}))?;
            tx.commit()?;
            fence.ended.insert((actor,ticket));
            changed.send_modify(|v| *v=v.wrapping_add(1));
            Ok(())
        }).await
    }
    /// Local trusted hub-coglet adapter only. `runtime_offset` is an explicitly
    /// established signed offset from hub to receiving runtime logical epoch.
    /// Clock discovery is not part of ALPN 0.1. No established map => no first send.
    /// An existing ID returns the original mapped request without a new mapping.
    /// The adapter must not use an old mapping after the receiver changes epochs.
    pub async fn prepare_action(
        &self,
        target: &str,
        action: Value,
        context: Value,
        runtime_offset: Option<i64>,
    ) -> Result<Value> {
        let target = target.to_owned();
        self.work(move|s| {
            if !valid("action",&action) || !valid("hub-context",&context) || action["coglet"]!=target { return Err(Error::Invalid); }
            let id=action["id"].as_str().ok_or(Error::Invalid)?.to_owned();
            let intent=json!({"action":action,"context":context});
            let old=s.store.db.query_row("SELECT intent,request FROM deliveries WHERE target=?1 AND id=?2",params![target,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?;
            if let Some((original,request))=old {
                if serde_json::from_str::<Value>(&original)?!=intent {return Err(Error::Conflict(0));}
                let request=serde_json::from_str(&request)?;
                s.action_authority(&target,&request)?;
                return Ok(request);
            }
            let offset=runtime_offset.ok_or(Error::Denied)?;
            let mut mapped=action.clone();
            mapped["deadline"]=json!(action["deadline"].as_u64().ok_or(Error::Invalid)?.checked_add_signed(offset).ok_or(Error::Invalid)?);
            let request=json!({"method":"hub.actions","target":target,"correlation_id":id,"timeout_ms":5000,"params":{"action":mapped},"context":context});
            s.action_authority(&target,&request)?;
            let tx=s.store.db.transaction()?;
            let pending:i64=tx.query_row("SELECT count(*) FROM deliveries WHERE target=?1 AND reply IS NULL",[&target],|r|r.get(0))?;
            if pending>=256 {return Err(Error::Full);}
            tx.execute("INSERT INTO deliveries(target,id,intent,request) VALUES(?1,?2,?3,?4)",params![target,id,serde_json::to_string(&intent)?,serde_json::to_string(&request)?])?;
            audit(&tx,json!({"method":"hub.actions","operation_id":id,"target":target,"actor":context["actor"],"ticket":context["ticket"],"outcome":"retained"}))?;
            tx.commit()?; Ok(request)
        }).await
    }
    /// Claims one dispatch durably before network I/O. A lost/ambiguous response
    /// fences replay, including after process restart. Recovery must reconcile the
    /// receiver receipt; it must never manufacture a new action ID or deadline.
    pub async fn dispatch_action(&self, target: &str, id: &str, session: Session) -> Result<Value> {
        self.dispatch_with_clock(target, id, session, None).await
    }
    /// Trusted local adapter only: the callback returns a fresh receiving-runtime
    /// logical time in the exact epoch of the retained mapped envelope. It must
    /// return None on clock loss, receiver restart/epoch change or uncertain mapping.
    /// It must be bounded/nonblocking and must not call back into this Kernel.
    /// No clock is persisted or inferred from Unix authority time. The receiver
    /// still owns final admission; this sender fence cannot undo delivered bytes.
    pub async fn dispatch_action_with_clock(
        &self,
        target: &str,
        id: &str,
        session: Session,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ) -> Result<Value> {
        self.dispatch_with_clock(target, id, session, Some(clock))
            .await
    }
    async fn dispatch_with_clock(
        &self,
        target: &str,
        id: &str,
        session: Session,
        clock: Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>,
    ) -> Result<Value> {
        // Reserve before retaining an attempt: local capacity refusal proves no
        // I/O happened and must never become durable unknown-effect uncertainty.
        let driver = Arc::new(
            self.dispatch
                .clone()
                .try_acquire_owned()
                .map_err(|_| Error::Full)?,
        );
        let key = (target.to_owned(), id.to_owned());
        {
            let mut fence = self.boundary.lock().map_err(|_| Error::Denied)?;
            if !fence.active.insert(key.clone()) {
                return Err(Error::Full);
            }
        }
        let _active = ActiveDispatch {
            boundary: self.boundary.clone(),
            key,
        };
        let target = target.to_owned();
        let id = id.to_owned();
        let t = target.clone();
        let i = id.clone();
        let peer = session.peer();
        let admission_clock = clock.clone();
        let (request,reply,attempted)=self.work(move|s| {
            let row=s.store.db.query_row("SELECT request,attempted,reply FROM deliveries WHERE target=?1 AND id=?2",params![t,i],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or(Error::Denied)?;
            let request:Value=serde_json::from_str(&row.0)?;
            let r=s.action_authority(&t,&request)?;
            if r.device!=peer.to_string() {return Err(Error::Denied);}
            if let Some(reply)=row.2 {return Ok((request,Some(serde_json::from_str(&reply)?),row.1!=0));}
            if row.1!=0 {return Ok((request,None,true));}
            check_action_deadline(&request, admission_clock.as_ref())?;
            let tx=s.store.db.transaction()?;
            tx.execute("UPDATE deliveries SET attempted=1 WHERE target=?1 AND id=?2",params![t,i])?;
            audit(&tx,json!({"method":"hub.actions.dispatch","operation_id":i,"target":t,"actor":request["context"]["actor"],"ticket":request["context"]["ticket"],"outcome":"attempted"}))?;
            tx.commit()?; Ok((request,None,false))
        }).await?;
        if let Some(reply) = reply {
            return Ok(reply);
        }
        let mut result = if attempted {
            None
        } else {
            self.request_fenced(&session, request.clone(), driver.clone(), clock)
                .await
                .ok()
        };
        if result.is_none() {
            // An ambiguous attempt is never resent. Query the original operation
            // under the original actor/ticket, with a fresh observation correlation.
            let receipt = json!({"method":"hub.receipt","target":target,"correlation_id":hex::encode(&iroh::SecretKey::generate().to_bytes()[..16]),"timeout_ms":5000,"params":{"operation_id":id},"context":request["context"]});
            result = self
                .request_fenced(&session, receipt, driver.clone(), None)
                .await
                .ok()
                .filter(|v| v["type"] == "reply" && v["result"]["operation_id"] == id);
        }
        let mut result=result.unwrap_or_else(|| {
            let mut v=failure(json!(id),Error::Denied);
            v["error"]=json!({"code":"unavailable","message":"action effect is unknown; not replayed","retryable":false,"details":{"unknown_effect":true}});v
        });
        // The local adapter always gets the original dispatch identity; preserve
        // the receiver's authoritative receipt rather than inventing action-accept.
        result["correlation_id"] = json!(id);
        self.work(move |s| s.finish_action(&target, &id, result))
            .await
    }
}
impl State {
    fn finish_action(&mut self, target: &str, id: &str, mut result: Value) -> Result<Value> {
        let tx = self.store.db.transaction()?;
        // A terminal local result committed while the transport reply was in
        // flight wins, including over a nonterminal queued receipt. Read and
        // audit in the same transaction as normal dispatch finalization.
        let saved: Option<String> = tx.query_row(
            "SELECT reply FROM deliveries WHERE target=?1 AND id=?2",
            params![target, id],
            |r| r.get(0),
        )?;
        if let Some(saved) = saved {
            tx.commit()?;
            return Ok(serde_json::from_str(&saved)?);
        }
        if result["result"]["state"] == "queued" {
            audit(
                &tx,
                json!({"method":"hub.actions.reconcile","operation_id":id,"target":target,"outcome":"queued"}),
            )?;
            tx.commit()?;
            return Ok(result);
        }
        if result["error"]["details"]["unknown_effect"] == true {
            let known: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM outcomes WHERE target=?1 AND action_id=?2)",
                params![target, id],
                |r| r.get(0),
            )?;
            if known {
                result["error"]["details"] =
                    json!({"unknown_effect":false,"outcome_observed":true});
                result["error"]["message"] =
                    json!("dispatch reply unavailable; durable action outcome observed");
            }
        }
        let changed = tx.execute(
            "UPDATE deliveries SET reply=?3 WHERE target=?1 AND id=?2 AND reply IS NULL",
            params![target, id, serde_json::to_string(&result)?],
        )?;
        if changed > 0 {
            audit(
                &tx,
                json!({"method":"hub.actions.dispatch","operation_id":id,"target":target,"outcome":if result["error"]["details"]["unknown_effect"]==true {"unknown_effect"} else {"replied"}}),
            )?;
        }
        let saved: String = tx.query_row(
            "SELECT reply FROM deliveries WHERE target=?1 AND id=?2",
            params![target, id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(serde_json::from_str(&saved)?)
    }
}
impl Kernel {
    async fn request_fenced(
        &self,
        session: &Session,
        request: Value,
        permit: Arc<tokio::sync::OwnedSemaphorePermit>,
        clock: Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>,
    ) -> Result<Value> {
        // Authority is synchronous SQLite, so drive the transport future on a
        // bounded blocking executor. No State/boundary lock survives a poll.
        // This budget is distinct from control work: stalled peers cannot consume
        // revocation's executor capacity. Dropping this caller cancels its driver.
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
                let target = request["target"].as_str().ok_or(Error::Denied)?.to_owned();
                // Receipt keeps the original action scope/capability policy, not
                // independent kernel authority under a substitute actor or ticket.
                let original: Value = {
                    let s = state.lock().map_err(|_| Error::Denied)?;
                    let id = if request["method"] == "hub.receipt" {
                        &request["params"]["operation_id"]
                    } else {
                        &request["correlation_id"]
                    };
                    let text: String = s.store.db.query_row(
                        "SELECT request FROM deliveries WHERE target=?1 AND id=?2",
                        params![target, id.as_str().ok_or(Error::Denied)?],
                        |r| r.get(0),
                    )?;
                    serde_json::from_str(&text)?
                };
                let scope = original["context"]["ticket"].as_str().map(|ticket| {
                    (
                        original["context"]["actor"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        ticket.to_owned(),
                    )
                });
                let is_action = request["method"] == "hub.actions";
                let cancel_view = cancelled.clone();
                let mut sending = Box::pin(session.request(request));
                loop {
                    let guarded = std::future::poll_fn(|cx| {
                        // Same state→boundary order as all operator paths. Shared
                        // Authority checks repeat after every async setup/write wake.
                        let s = state.lock().map_err(|_| Error::Denied)?;
                        let fence = boundary.lock().map_err(|_| Error::Denied)?;
                        if *cancel_view.borrow()
                            || fence.revoked.contains(&peer)
                            || scope.as_ref().is_some_and(|s| fence.ended.contains(s))
                        {
                            return std::task::Poll::Ready(Err(Error::Denied));
                        }
                        let r = s.action_authority(&target, &original)?;
                        if r.device != peer.to_string() {
                            return std::task::Poll::Ready(Err(Error::Denied));
                        }
                        // Sample only AFTER all blocking authority/database work.
                        // The retained mapped deadline is exclusive. Receipt reads
                        // remain legal after expiry and never resend the action.
                        if is_action {
                            check_action_deadline(&original, clock.as_ref())?;
                        }
                        sending
                            .as_mut()
                            .poll(cx)
                            .map(|r| r.map_err(|_| Error::Denied))
                    });
                    tokio::select! {
                        biased;
                        _ = cancelled.changed() => return Err(Error::Denied),
                        _ = changed.changed() => continue,
                        result = guarded => return result,
                        // Expiry/clock loss must also fence a request with no I/O wake.
                        _ = tokio::time::sleep(Duration::from_millis(25)) => continue,
                    }
                }
            })
        })
        .await
        .map_err(|_| Error::Denied)?
    }
}
