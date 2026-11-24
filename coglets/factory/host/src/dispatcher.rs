use crate::adapter::{AdapterError, FollowUp, HermesAdapter, Outcome, RemoteTask, TaskSpec};
use coglet_db::{Database, OutboxRecord, OutboxState, Result, WorkQueueRow};
use coglet_types::Value;

pub struct Dispatcher<A> {
    pub adapter: A,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchBatch {
    pub claimed: usize,
    pub follow_ups: Vec<FollowUp>,
}

impl<A: HermesAdapter> Dispatcher<A> {
    pub fn new(adapter: A) -> Self {
        Self { adapter }
    }

    /// Claim fenced intents and submit them with stable idempotency keys.
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
            follow_ups,
        })
    }

    /// After restart: inflight rows become uncertain, then the same keys retry.
    pub fn recover_and_dispatch(
        &mut self,
        db: &mut Database,
        now: i64,
        limit: u32,
    ) -> Result<DispatchBatch> {
        db.reconcile_inflight_outbox()?;
        self.dispatch_pending(db, now, limit)
    }

    fn submit(
        &mut self,
        db: &mut Database,
        row: &OutboxRecord,
        now: i64,
    ) -> Result<Option<FollowUp>> {
        let task_id = task_id_of(row);
        let spec = TaskSpec {
            idempotency_key: row.idempotency_key.clone(),
            kind: row.kind.clone(),
            payload_cbor_hex: row.payload_cbor_hex.clone(),
            correlation_id: row.correlation_id.clone(),
            task_id: task_id.clone(),
        };
        match self.adapter.create(&spec) {
            Ok(remote) => {
                self.record_queue(db, &task_id, &remote, row.attempt, now)?;
                Ok(None)
            }
            Err(AdapterError::Uncertain) => {
                db.record_outbox_state(&row.intent_id, row.version, OutboxState::Uncertain)?;
                Ok(None)
            }
            Err(AdapterError::Failed(note)) => {
                db.record_outbox_state(&row.intent_id, row.version, OutboxState::Failed)?;
                Ok(Some(FollowUp {
                    task_id,
                    kind: "task.failed".into(),
                    note,
                }))
            }
            Err(AdapterError::Conflict(note)) => Err(note),
        }
    }

    fn record_queue(
        &mut self,
        db: &mut Database,
        task_id: &str,
        remote: &RemoteTask,
        attempt: u64,
        now: i64,
    ) -> Result<()> {
        let current = db.get_work_queue(task_id)?;
        let version = current.as_ref().map(|r| r.version).unwrap_or(1);
        let attempt = current
            .as_ref()
            .map(|r| r.attempt.max(attempt))
            .unwrap_or(attempt);
        let row = WorkQueueRow {
            task_id: task_id.into(),
            state: "dispatched".into(),
            priority: 0,
            lease_owner: Some(remote.remote_id.clone()),
            lease_until: Some(now.saturating_add(60_000)),
            attempt,
            version: if current.is_some() { version } else { 1 },
        };
        db.put_work_queue(&row)?;
        Ok(())
    }

    /// Apply a later worker result. Outcomes are immutable events, not in-place
    /// mutation of cognitive objects (the Factory executive consumes them).
    pub fn apply_result(
        &mut self,
        db: &mut Database,
        intent_id: &str,
        outcome: Outcome,
    ) -> Result<FollowUp> {
        let row = db
            .get_outbox(intent_id)?
            .ok_or_else(|| "unknown intent".to_string())?;
        if row.state != OutboxState::Dispatching.as_str() {
            return Err("intent not dispatching".into());
        }
        let task_id = task_id_of(&row);
        let (state, kind, note) = match outcome {
            Outcome::Succeeded { note } => (OutboxState::Succeeded, "task.completed", note),
            Outcome::Failed { note } => (OutboxState::Failed, "task.failed", note),
        };
        db.record_outbox_state(&row.intent_id, row.version, state)?;
        if let Some(queue) = db.get_work_queue(&task_id)? {
            db.put_work_queue(&WorkQueueRow {
                state: "done".into(),
                lease_owner: None,
                lease_until: None,
                version: queue.version,
                ..queue
            })?;
        }
        Ok(FollowUp {
            task_id,
            kind: kind.into(),
            note,
        })
    }

    pub fn cancel_intent(&mut self, db: &mut Database, intent_id: &str) -> Result<FollowUp> {
        let row = db
            .get_outbox(intent_id)?
            .ok_or_else(|| "unknown intent".to_string())?;
        if row.state != OutboxState::Dispatching.as_str() {
            return Err("intent not dispatching".into());
        }
        if let Some(queue) = db.get_work_queue(&task_id_of(&row))?
            && let Some(remote) = queue.lease_owner.as_deref()
        {
            self.adapter.cancel(remote).map_err(|e| format!("{e:?}"))?;
        }
        db.record_outbox_state(&row.intent_id, row.version, OutboxState::Cancelled)?;
        Ok(FollowUp {
            task_id: task_id_of(&row),
            kind: "task.cancelled".into(),
            note: "cancelled".into(),
        })
    }

    pub fn expire_leases(&mut self, db: &mut Database, now: i64) -> Result<Vec<String>> {
        db.expire_work_leases(now)
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
