//! Provisional dedicated enqueue. No dispatch or successful public commit path.
use super::*;
use coglet_types::Value as V;
use rusqlite::OptionalExtension;

pub struct Intent {
    pub intent_id: String,
    pub idempotency_key: String,
    pub kind: String,
    pub payload: V,
    pub correlation_id: String,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Enqueued {
    pub intent_id: String,
    pub duplicate: bool,
}
impl Unit<'_> {
    /// Host-only configuration, pinned before any call. An admitted unit already
    /// owns its complete grant snapshot and cannot be reconfigured.
    pub fn with_intent_grants(mut self, kinds: impl IntoIterator<Item = String>) -> Result<Self> {
        if self.calls != 0 || self.intent_grants.is_some() || self.poisoned {
            return Err("intent grants already pinned".into());
        }
        self.intent_grants = Some(kinds.into_iter().collect());
        Ok(self)
    }
    pub fn enqueue(&mut self, intent: &Intent) -> Result<Enqueued> {
        self.calls = self.calls.saturating_add(1);
        let result = self.enqueue_inner(intent);
        let result = if result.is_ok() && Instant::now() >= self.deadline {
            Err("resource exhausted".into())
        } else {
            result
        };
        if result.is_err() {
            self.poisoned = true;
            self.rollback_failed_unit();
        }
        result
    }
    /// Identity-only read. Call only after shape, grants and poison validation;
    /// the wire entry also reserves output before this DB access. The owner lease
    /// prevents another admitted unit/dispatcher from changing identity in between
    /// preflight and execution. No application mutation or transaction is started.
    pub(super) fn enqueue_identity(&self, intent: &Intent) -> Result<Option<Enqueued>> {
        let payload = hex::encode(coglet_types::encode_profile(&intent.payload)?);
        let existing: Option<(String,String,String,String)> = self.db.connection.query_row(
            "SELECT intent_id,kind,payload_cbor_hex,correlation_id FROM outbox WHERE idempotency_key=?1",
            [&intent.idempotency_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
        ).optional().map_err(sqlite_failure)?;
        if let Some((id, kind, old_payload, correlation)) = existing {
            if kind != intent.kind || old_payload != payload || correlation != intent.correlation_id
            {
                return Err("idempotency conflict".into());
            }
            // Original key/content wins even if the proposed ID belongs elsewhere.
            return Ok(Some(Enqueued {
                intent_id: id,
                duplicate: true,
            }));
        }
        let reused: bool = self
            .db
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM outbox WHERE intent_id=?1)",
                [&intent.intent_id],
                |r| r.get(0),
            )
            .map_err(sqlite_failure)?;
        if reused {
            return Err("intent ID conflict".into());
        }
        Ok(None)
    }
    fn enqueue_inner(&mut self, intent: &Intent) -> Result<Enqueued> {
        let text = |s: &str| V::Text(s.into());
        let request = V::Map(vec![
            (text("operation"), text("enqueue")),
            (text("intent_id"), text(&intent.intent_id)),
            (text("idempotency_key"), text(&intent.idempotency_key)),
            (text("kind"), text(&intent.kind)),
            (text("payload"), intent.payload.clone()),
            (text("correlation_id"), text(&intent.correlation_id)),
        ]);
        let bytes = coglet_types::encode_typed("database::db-enqueue", &request)?;
        if bytes.len() > 65536 {
            return Err("invalid request bounds".into());
        }
        if !self.grants.write
            || self
                .intent_grants
                .as_ref()
                .is_none_or(|k| !k.contains(&intent.kind))
        {
            return Err("permission denied".into());
        }
        if self.poisoned {
            return Err("poisoned unit".into());
        }
        if self.paging {
            return Err("paging conflict".into());
        }
        // Identity conflicts precede fresh admission; an identical retry still
        // cannot bypass the absolute deadline or the 32-call ceiling.
        let duplicate = self.enqueue_identity(intent)?;
        if self.calls > 32 || Instant::now() >= self.deadline {
            return Err("resource exhausted".into());
        }
        if let Some(reply) = duplicate {
            return Ok(reply);
        }
        if self.pins.is_some() && self.revision == u64::MAX {
            return Err("revision overflow".into());
        }
        self.start_transaction()?;
        let payload = hex::encode(coglet_types::encode_profile(&intent.payload)?);
        self.db.connection.execute(
            "INSERT INTO outbox(intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version) VALUES (?1,?2,?3,?4,?5,'pending',0,1)",
            rusqlite::params![intent.intent_id,intent.idempotency_key,intent.kind,payload,intent.correlation_id],
        ).map_err(sqlite_failure)?;
        self.wrote = true;
        Ok(Enqueued {
            intent_id: intent.intent_id.clone(),
            duplicate: false,
        })
    }
}
