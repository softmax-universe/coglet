use crate::adapter::{AdapterError, FollowUp, HermesAdapter, Outcome, WorkerReport};
use coglet_db::{Database, OutboxRecord, OutboxState, Result, WorkQueueRow};
use coglet_types::Value;
use std::collections::BTreeSet;

const HOST_OWNER: &str = "factory-host";
const LEASE_MS: i64 = 60_000;

pub struct Dispatcher<A> {
    pub adapter: A,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchBatch {
    pub claimed: usize,
    pub expired: Vec<String>,
    pub follow_ups: Vec<FollowUp>,
}

/// Host entry. Production constructs this with [`crate::HttpKanban`]; tests may
/// substitute [`crate::MockAdapter`]. The cycle never reads Hermes tables.
pub struct FactoryHost<A> {
    dispatcher: Dispatcher<A>,
}

impl FactoryHost<crate::HttpKanban> {
    pub fn connect(base_url: &str) -> Self {
        Self {
            dispatcher: Dispatcher::new(crate::HttpKanban::new(base_url)),
        }
    }
}

impl<A: HermesAdapter> FactoryHost<A> {
    pub fn from_adapter(adapter: A) -> Self {
        Self {
            dispatcher: Dispatcher::new(adapter),
        }
    }

    pub fn run(&mut self, db: &mut Database, now: i64) -> Result<DispatchBatch> {
        self.dispatcher.run_cycle(db, now, 8)
    }

    pub fn dispatcher(&mut self) -> &mut Dispatcher<A> {
        &mut self.dispatcher
    }
}

impl<A: HermesAdapter> Dispatcher<A> {
    pub fn new(adapter: A) -> Self {
        Self { adapter }
    }

    /// Recover unbound creates, finish bound claims that never leased, expire
    /// leases, redispatch, claim new intents, and ingest worker results.
    /// Pending deliveries are returned until acknowledged.
    pub fn run_cycle(&mut self, db: &mut Database, now: i64, limit: u32) -> Result<DispatchBatch> {
        let reconciled = self.reconcile_bound_unleased(db, now)?;
        let mut follow_ups = self.pending_follow_ups(db)?;
        // A durable result may precede local lease expiry. Consume it before
        // unsupported remote retry can hold the remaining operational backlog.
        follow_ups.extend(self.ingest(db, now)?);
        let expired = self.expire_leases(db, now)?;
        let rebound = self.redispatch_pending(db, now, limit)?;
        let fresh = self.dispatch_pending(db, now, limit)?;
        follow_ups.extend(self.ingest(db, now)?);
        follow_ups.extend(rebound.follow_ups);
        follow_ups.extend(fresh.follow_ups);
        follow_ups = dedupe(follow_ups);
        Ok(DispatchBatch {
            claimed: fresh.claimed + rebound.claimed + reconciled,
            expired,
            follow_ups,
        })
    }

    pub fn dispatch_pending(
        &mut self,
        db: &mut Database,
        now: i64,
        limit: u32,
    ) -> Result<DispatchBatch> {
        let claimed = db.claim_pending_outbox(limit)?;
        let n = claimed.len();
        let mut follow_ups = Vec::new();
        for row in claimed {
            if let Some(event) = self.submit(db, &row, now)? {
                follow_ups.push(event);
            }
        }
        Ok(DispatchBatch {
            claimed: n,
            expired: vec![],
            follow_ups,
        })
    }

    pub fn recover_and_dispatch(
        &mut self,
        db: &mut Database,
        now: i64,
        limit: u32,
    ) -> Result<DispatchBatch> {
        for row in db.unbound_dispatching()? {
            db.record_outbox_state(&row.intent_id, row.version, OutboxState::Uncertain)?;
        }
        self.run_cycle(db, now, limit)
    }

    fn submit(
        &mut self,
        db: &mut Database,
        row: &OutboxRecord,
        now: i64,
    ) -> Result<Option<FollowUp>> {
        let task_id = task_id_of(row);
        let spec = crate::adapter::TaskSpec {
            idempotency_key: row.idempotency_key.clone(),
            kind: row.kind.clone(),
            payload_cbor_hex: row.payload_cbor_hex.clone(),
            correlation_id: row.correlation_id.clone(),
            task_id: task_id.clone(),
        };
        match self.adapter.create(&spec) {
            Ok(remote) => {
                db.put_binding(&row.intent_id, &remote.remote_id, &task_id)?;
                self.lease(db, row, &remote.remote_id, &task_id, row.attempt, now)?;
                Ok(None)
            }
            Err(AdapterError::Uncertain) => {
                db.record_outbox_state(&row.intent_id, row.version, OutboxState::Uncertain)?;
                Ok(None)
            }
            Err(AdapterError::Failed(note)) => Ok(Some(self.finish(
                db,
                row,
                &task_id,
                OutboxState::Failed,
                "task.failed",
                &note,
                None,
                now,
            )?)),
            Err(AdapterError::Conflict(note)) => Err(note),
        }
    }

    fn lease(
        &mut self,
        db: &mut Database,
        row: &OutboxRecord,
        remote_id: &str,
        task_id: &str,
        attempt: u64,
        now: i64,
    ) -> Result<()> {
        // A renewed attempt must claim again. The stored token is only a
        // crash marker for a lease that was never written locally.
        db.set_binding_phase(&row.intent_id, "claiming", "")?;
        let until = now.saturating_add(LEASE_MS);
        let claimed = match self
            .adapter
            .claim(remote_id, HOST_OWNER, until as u64, attempt)
        {
            Ok(lease) => lease,
            Err(AdapterError::Uncertain) => return Ok(()),
            Err(AdapterError::Failed(note) | AdapterError::Conflict(note)) => return Err(note),
        };
        db.set_binding_phase(&row.intent_id, "claimed", &claimed.token)?;
        self.persist_local_lease(db, task_id, &claimed.token, attempt, claimed.until as i64)
    }

    fn persist_local_lease(
        &mut self,
        db: &mut Database,
        task_id: &str,
        token: &str,
        attempt: u64,
        until: i64,
    ) -> Result<()> {
        let current = db.get_work_queue(task_id)?;
        if current.as_ref().is_some_and(|row| row.state == "leased") {
            return Ok(());
        }
        let version = current.as_ref().map(|row| row.version).unwrap_or(1);
        let priority = current.as_ref().map(|row| row.priority).unwrap_or(0);
        db.put_work_queue(&WorkQueueRow {
            task_id: task_id.into(),
            state: "leased".into(),
            priority,
            lease_owner: Some(token.into()),
            lease_until: Some(until),
            attempt,
            version: if current.is_some() { version } else { 1 },
        })?;
        Ok(())
    }

    /// Dispatching rows that already have a remote id but no local lease.
    /// Covers a crash after binding, after an uncertain claim, and after the
    /// claim token was saved but before `work_queue` was written.
    fn reconcile_bound_unleased(&mut self, db: &mut Database, now: i64) -> Result<usize> {
        let mut finished = 0;
        for row in db.outbox_in_state(OutboxState::Dispatching.as_str())? {
            let Some(binding) = db.get_binding_record(&row.intent_id)? else {
                continue;
            };
            let task_id = task_id_of(&row);
            if db.get_work_queue(&task_id)?.is_some() {
                continue;
            }
            if binding.phase == "claimed" && !binding.token.is_empty() {
                // The remote run may have finished before the crashed host
                // wrote its local lease. Reconcile that exact saved run first;
                // a terminal run cannot (and must not) be renewed/reclaimed.
                match self
                    .adapter
                    .poll(&binding.remote_id, row.attempt, &binding.token)
                {
                    Ok(Some(report)) => {
                        self.persist_local_lease(
                            db,
                            &task_id,
                            &binding.token,
                            row.attempt,
                            now.saturating_add(LEASE_MS),
                        )?;
                        self.apply_result(db, &row.intent_id, report)?;
                        finished += 1;
                        continue;
                    }
                    Ok(None) => {}
                    Err(AdapterError::Uncertain) => continue,
                    Err(AdapterError::Failed(note) | AdapterError::Conflict(note)) => {
                        return Err(note);
                    }
                }
                let restored = match self.adapter.restore_lease(
                    &binding.remote_id,
                    HOST_OWNER,
                    now.saturating_add(LEASE_MS) as u64,
                    row.attempt,
                    &binding.token,
                ) {
                    Ok(lease) => lease,
                    Err(AdapterError::Uncertain) => continue,
                    Err(AdapterError::Failed(note) | AdapterError::Conflict(note)) => {
                        return Err(note);
                    }
                };
                self.persist_local_lease(
                    db,
                    &task_id,
                    &binding.token,
                    row.attempt,
                    restored.until as i64,
                )?;
            } else {
                let spec = crate::adapter::TaskSpec {
                    idempotency_key: row.idempotency_key.clone(),
                    kind: row.kind.clone(),
                    payload_cbor_hex: row.payload_cbor_hex.clone(),
                    correlation_id: row.correlation_id.clone(),
                    task_id: task_id.clone(),
                };
                let remote = match self.adapter.create(&spec) {
                    Ok(remote) => remote,
                    Err(AdapterError::Uncertain) => continue,
                    Err(AdapterError::Failed(note) | AdapterError::Conflict(note)) => {
                        return Err(note);
                    }
                };
                if remote.remote_id != binding.remote_id {
                    return Err("binding remote mismatch".into());
                }
                self.lease(db, &row, &remote.remote_id, &task_id, row.attempt, now)?;
            }
            if db
                .get_work_queue(&task_id)?
                .is_some_and(|queue| queue.state == "leased")
            {
                finished += 1;
            }
        }
        Ok(finished)
    }

    pub fn apply_result(
        &mut self,
        db: &mut Database,
        intent_id: &str,
        report: WorkerReport,
    ) -> Result<FollowUp> {
        let row = db
            .get_outbox(intent_id)?
            .ok_or_else(|| "unknown intent".to_string())?;
        let task_id = task_id_of(&row);
        let (state, kind, note) = match &report.outcome {
            Outcome::Succeeded { note } => (OutboxState::Succeeded, "task.completed", note.clone()),
            Outcome::Failed { note } => (OutboxState::Failed, "task.failed", note.clone()),
        };
        if row.state == state.as_str() {
            return self
                .pending_follow_ups(db)?
                .into_iter()
                .find(|f| f.task_id == task_id && f.kind == kind)
                .ok_or_else(|| "terminal outcome has no delivery".to_string());
        }
        if row.state != OutboxState::Dispatching.as_str() {
            return Err("intent not dispatching".into());
        }
        self.finish(
            db,
            &row,
            &task_id,
            state,
            kind,
            &note,
            Some((report.lease.as_str(), report.attempt)),
            0,
        )
    }

    pub fn cancel_intent(
        &mut self,
        db: &mut Database,
        intent_id: &str,
        attempt: u64,
        lease: &str,
        now: i64,
    ) -> Result<FollowUp> {
        let row = db
            .get_outbox(intent_id)?
            .ok_or_else(|| "unknown intent".to_string())?;
        let task_id = task_id_of(&row);
        self.fence(db, &task_id, attempt, lease)?;
        if let Some(remote) = db.get_binding(intent_id)? {
            self.adapter
                .cancel(&remote, attempt, lease)
                .map_err(|e| format!("{e:?}"))?;
        }
        self.finish(
            db,
            &row,
            &task_id,
            OutboxState::Cancelled,
            "task.cancelled",
            "cancelled",
            Some((lease, attempt)),
            now,
        )
    }

    pub fn heartbeat(
        &mut self,
        db: &mut Database,
        intent_id: &str,
        attempt: u64,
        lease: &str,
        now: i64,
    ) -> Result<()> {
        let row = db
            .get_outbox(intent_id)?
            .ok_or_else(|| "unknown intent".to_string())?;
        let task_id = task_id_of(&row);
        let queue = self.fence(db, &task_id, attempt, lease)?;
        let mut until = now.saturating_add(LEASE_MS);
        if let Some(remote) = db.get_binding(intent_id)? {
            until = self
                .adapter
                .heartbeat(&remote, until as u64, attempt, lease)
                .map_err(|e| format!("{e:?}"))? as i64;
        }
        db.put_work_queue(&WorkQueueRow {
            lease_until: Some(until),
            version: queue.version,
            ..queue
        })?;
        Ok(())
    }

    pub fn progress(
        &mut self,
        db: &mut Database,
        intent_id: &str,
        attempt: u64,
        lease: &str,
        note: &str,
    ) -> Result<()> {
        let row = db
            .get_outbox(intent_id)?
            .ok_or_else(|| "unknown intent".to_string())?;
        self.fence(db, &task_id_of(&row), attempt, lease)?;
        if let Some(remote) = db.get_binding(intent_id)? {
            self.adapter
                .progress(&remote, note, attempt, lease)
                .map_err(|e| format!("{e:?}"))?;
        }
        Ok(())
    }

    pub fn expire_leases(&mut self, db: &mut Database, now: i64) -> Result<Vec<String>> {
        if !self.adapter.supports_retry() {
            return Ok(vec![]);
        }
        let due = db.work_in_state("leased")?;
        let mut expired = Vec::new();
        for row in due {
            if row.lease_until.is_some_and(|until| until <= now) {
                if let Some(intent) = intent_for_task(db, &row.task_id)?
                    && let Some(remote) = db.get_binding(&intent)?
                {
                    match self.adapter.retry(&remote) {
                        Ok(()) | Err(AdapterError::Uncertain) => {}
                        Err(AdapterError::Failed(note) | AdapterError::Conflict(note)) => {
                            return Err(note);
                        }
                    }
                }
                expired.push(row.task_id);
            }
        }
        if expired.is_empty() {
            return Ok(expired);
        }
        let cleared = db.expire_work_leases(now)?;
        Ok(cleared)
    }

    pub fn redispatch_pending(
        &mut self,
        db: &mut Database,
        now: i64,
        limit: u32,
    ) -> Result<DispatchBatch> {
        let mut claimed = 0;
        for row in db.work_in_state("pending")? {
            if claimed >= limit {
                break;
            }
            let Some(intent) = intent_for_task(db, &row.task_id)? else {
                continue;
            };
            let Some(remote) = db.get_binding(&intent)? else {
                continue;
            };
            let outbox = db
                .get_outbox(&intent)?
                .ok_or_else(|| "missing outbox".to_string())?;
            if outbox.state != OutboxState::Dispatching.as_str() {
                continue;
            }
            self.lease(db, &outbox, &remote, &row.task_id, row.attempt, now)?;
            claimed += 1;
        }
        Ok(DispatchBatch {
            claimed: claimed as usize,
            expired: vec![],
            follow_ups: vec![],
        })
    }

    fn ingest(&mut self, db: &mut Database, now: i64) -> Result<Vec<FollowUp>> {
        let mut events = Vec::new();
        for row in db.outbox_in_state(OutboxState::Dispatching.as_str())? {
            let Some(remote) = db.get_binding(&row.intent_id)? else {
                continue;
            };
            let Some(queue) = db.get_work_queue(&task_id_of(&row))? else {
                continue;
            };
            let Some(token) = queue.lease_owner.as_deref() else {
                continue;
            };
            let report = match self.adapter.poll(&remote, queue.attempt, token) {
                Ok(Some(report)) => report,
                Ok(None) => continue,
                Err(AdapterError::Uncertain) => continue,
                Err(AdapterError::Failed(note) | AdapterError::Conflict(note)) => {
                    return Err(note);
                }
            };
            match self.apply_result(db, &row.intent_id, report) {
                Ok(follow) => events.push(follow),
                Err(error) if error == "stale attempt" => {}
                Err(error) => return Err(error),
            }
        }
        let _ = now;
        Ok(events)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        db: &mut Database,
        row: &OutboxRecord,
        task_id: &str,
        state: OutboxState,
        kind: &str,
        note: &str,
        lease: Option<(&str, u64)>,
        now: i64,
    ) -> Result<FollowUp> {
        let attempt = lease.map(|(_, attempt)| attempt).unwrap_or(row.attempt);
        let token = lease.map(|(token, _)| token).unwrap_or("");
        let event_id = event_id(&row.intent_id, attempt, kind);
        let payload = serde_json::to_string(&serde_json::json!({
            "task_id": task_id,
            "note": note,
            "attempt": attempt,
            "lease": token,
            "kind": kind,
        }))
        .map_err(|error| error.to_string())?;
        db.commit_outcome(
            &row.intent_id,
            row.version,
            state,
            &event_id,
            task_id,
            kind,
            &payload,
            now,
            lease,
        )?;
        Ok(FollowUp {
            event_id,
            task_id: task_id.into(),
            kind: kind.into(),
            note: note.into(),
            attempt,
            lease: token.into(),
        })
    }

    fn fence(
        &self,
        db: &Database,
        task_id: &str,
        attempt: u64,
        lease: &str,
    ) -> Result<WorkQueueRow> {
        let queue = db
            .get_work_queue(task_id)?
            .ok_or_else(|| "missing work_queue".to_string())?;
        if queue.state != "leased"
            || queue.attempt != attempt
            || queue.lease_owner.as_deref() != Some(lease)
        {
            return Err("stale attempt".into());
        }
        Ok(queue)
    }

    fn pending_follow_ups(&self, db: &Database) -> Result<Vec<FollowUp>> {
        db.pending_deliveries()?
            .into_iter()
            .map(|row| parse_follow_up(&row.event_id, &row.payload_json))
            .collect()
    }

    pub fn acknowledge(&mut self, db: &mut Database, event_id: &str) -> Result<()> {
        db.acknowledge_delivery(event_id)
    }
}

fn dedupe(items: Vec<FollowUp>) -> Vec<FollowUp> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.event_id.clone()))
        .collect()
}

fn intent_for_task(db: &Database, task_id: &str) -> Result<Option<String>> {
    for state in ["dispatching", "pending", "uncertain"] {
        for row in db.outbox_in_state(state)? {
            if task_id_of(&row) == task_id {
                return Ok(Some(row.intent_id));
            }
        }
    }
    Ok(None)
}

fn event_id(intent: &str, attempt: u64, kind: &str) -> String {
    format!(
        "e{attempt:02x}{kind_tag}{intent}",
        kind_tag = kind_tag(kind),
        intent = &intent[..30]
    )
}

fn kind_tag(kind: &str) -> char {
    match kind {
        "task.completed" => 'c',
        "task.failed" => 'f',
        "task.cancelled" => 'x',
        _ => 'o',
    }
}

fn parse_follow_up(event_id: &str, payload: &str) -> Result<FollowUp> {
    let value: serde_json::Value =
        serde_json::from_str(payload).map_err(|_| "delivery json".to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "delivery json".to_string())?;
    let text = |key: &str| -> Result<String> {
        object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("delivery {key}"))
    };
    let attempt = object
        .get("attempt")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "delivery attempt".to_string())?;
    Ok(FollowUp {
        event_id: event_id.into(),
        task_id: text("task_id")?,
        kind: text("kind").unwrap_or_else(|_| kind_from_event(event_id).into()),
        note: text("note")?,
        attempt,
        lease: text("lease")?,
    })
}

fn kind_from_event(event_id: &str) -> &'static str {
    match event_id.chars().nth(3) {
        Some('c') => "task.completed",
        Some('f') => "task.failed",
        Some('x') => "task.cancelled",
        _ => "task.failed",
    }
}

fn task_id_of(row: &OutboxRecord) -> String {
    if let Ok(bytes) = decode_hex(&row.payload_cbor_hex)
        && let Ok(id) = task_id_from_profile(&bytes)
    {
        return id;
    }
    row.correlation_id.clone()
}

fn decode_hex(text: &str) -> std::result::Result<Vec<u8>, ()> {
    if !text.len().is_multiple_of(2) {
        return Err(());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

fn task_id_from_profile(bytes: &[u8]) -> std::result::Result<String, ()> {
    let Value::Map(entries) = coglet_types::decode_profile(bytes).map_err(|_| ())? else {
        return Err(());
    };
    entries
        .iter()
        .find_map(|(key, value)| {
            (key.as_text() == Some("task_id")).then(|| value.as_text().map(str::to_owned))
        })
        .flatten()
        .ok_or(())
}
