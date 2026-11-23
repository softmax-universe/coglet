use super::*;
use coglet_proto::transport::Session;

impl State {
    fn action_authority(&self, target: &str, request: &Value) -> Result<Registration> {
        let r = self.registration(target)?;
        let peer = r.device.parse().map_err(|_| Error::Denied)?;
        self.authorize(peer, target, "hub.actions")?;
        let c = &request["context"];
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
        let _boundary = self.boundary.lock().await;
        let target = target.to_owned();
        let id = id.to_owned();
        let t = target.clone();
        let i = id.clone();
        let peer = session.peer();
        let (request,reply)=self.work(move|s| {
            let row=s.store.db.query_row("SELECT request,attempted,reply FROM deliveries WHERE target=?1 AND id=?2",params![t,i],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or(Error::Denied)?;
            let request:Value=serde_json::from_str(&row.0)?;
            let r=s.action_authority(&t,&request)?;
            if r.device!=peer.to_string() {return Err(Error::Denied);}
            if let Some(reply)=row.2 {return Ok((request,Some(serde_json::from_str(&reply)?)));}
            if row.1!=0 {return Err(Error::Denied);}
            let tx=s.store.db.transaction()?;
            tx.execute("UPDATE deliveries SET attempted=1 WHERE target=?1 AND id=?2",params![t,i])?;
            audit(&tx,json!({"method":"hub.actions.dispatch","operation_id":i,"target":t,"actor":request["context"]["actor"],"ticket":request["context"]["ticket"],"outcome":"attempted"}))?;
            tx.commit()?; Ok((request,None))
        }).await?;
        if let Some(reply) = reply {
            return Ok(reply);
        }
        let result = session.request(request).await.map_err(|_| Error::Denied)?;
        let returned = result.clone();
        self.work(move|s| {
            let tx=s.store.db.transaction()?;
            tx.execute("UPDATE deliveries SET reply=?3 WHERE target=?1 AND id=?2",params![target,id,serde_json::to_string(&result)?])?;
            audit(&tx,json!({"method":"hub.actions.dispatch","operation_id":id,"target":target,"outcome":"replied"}))?;
            tx.commit()?; Ok(())
        }).await?;
        Ok(returned)
    }
}
