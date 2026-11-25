//! Guest-issued, separately granted cancellation effects. Never infer authority
//! from goal audit rows. Held/unsupported cancellation must not stop siblings.
use super::*;
use sha2::{Digest, Sha256};

fn cancel_id(intent: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"factory-v1");
    for part in ["factory-cancel", intent] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn field(row: &OutboxRecord, key: &str) -> Result<String> {
    let bytes = decode_hex(&row.payload_cbor_hex).map_err(|_| "cancel payload")?;
    let value = coglet_types::decode_profile(&bytes).map_err(|_| "cancel payload")?;
    value
        .as_map()
        .and_then(|entries| {
            entries.iter().find_map(|(k, v)| {
                (k.as_text() == Some(key))
                    .then(|| v.as_text().map(str::to_owned))
                    .flatten()
            })
        })
        .ok_or_else(|| format!("cancel {key}"))
}

pub(super) fn requested(db: &Database, row: &OutboxRecord) -> Result<bool> {
    if row.kind != "factory.dispatch" {
        return Ok(false);
    }
    let Some(cancel) = db.get_outbox(&cancel_id(&row.intent_id))? else {
        return Ok(false);
    };
    if cancel.kind != "factory.cancel"
        || field(&cancel, "target_intent")? != row.intent_id
        || field(&cancel, "target_task")? != task_id_of(row)
    {
        return Err("cancel identity mismatch".into());
    }
    Ok(true)
}

impl<A: HermesAdapter> Dispatcher<A> {
    pub(super) fn consume_cancellations(&mut self, db: &mut Database, now: i64) -> Result<()> {
        for cancel in db.outbox_in_state("dispatching")? {
            if cancel.kind != "factory.cancel" {
                continue;
            }
            let target = field(&cancel, "target_intent")?;
            let Some(row) = db.get_outbox(&target)? else {
                continue;
            };
            if !requested(db, &row)? {
                return Err("invalid cancellation target".into());
            }
            if matches!(row.state.as_str(), "succeeded" | "failed" | "cancelled") {
                db.record_outbox_state(&cancel.intent_id, cancel.version, OutboxState::Succeeded)?;
                continue;
            }
            let Some(queue) = db.get_work_queue(&task_id_of(&row))? else {
                continue;
            };
            let attempt: u64 = field(&cancel, "attempt")?
                .parse()
                .map_err(|_| "cancel attempt")?;
            // A delayed attempt cancellation cannot acquire authority over a new
            // lease. Retain the request to suppress new submits of this intent.
            if queue.state != "leased"
                || queue.attempt != attempt
                || !queue.lease_until.is_some_and(|until| until > now)
                || !self.adapter.supports_cancel()
            {
                continue;
            }
            let Some(token) = queue.lease_owner.as_deref() else {
                continue;
            };
            db.dispatch_fence()?;
            self.effect_fence(db, &queue.task_id, attempt, token)?;
            let Some(remote) = db.get_binding(&row.intent_id)? else {
                continue;
            };
            match self.adapter.cancel(&remote, attempt, token) {
                Ok(()) => {
                    self.finish(
                        db,
                        &row,
                        &queue.task_id,
                        OutboxState::Cancelled,
                        "task.cancelled",
                        "cancelled",
                        Some((token, attempt)),
                        now,
                    )?;
                    db.record_outbox_state(
                        &cancel.intent_id,
                        cancel.version,
                        OutboxState::Succeeded,
                    )?;
                }
                // Neither transport failure nor unsupported operation proves
                // cancellation. Keep original result correlation and capacity.
                Err(_) => continue,
            }
        }
        Ok(())
    }
}
