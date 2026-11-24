//! Typed host dispatcher API over `dispatcher_mutation`. No arbitrary SQL
//! closures are exposed. Application outbox/work_queue tables are Factory schema.
use super::*;
use rusqlite::OptionalExtension;

impl Database {
    /// Claim pending or uncertain intents. Identity (intent_id, idempotency_key)
    /// is preserved; attempt and version increment. Limit is 1..=32.
    pub fn claim_pending_outbox(&mut self, limit: u32) -> Result<Vec<OutboxRecord>> {
        if !(1..=32).contains(&limit) {
            return Err("invalid outbox claim limit".into());
        }
        let mut claimed = Vec::new();
        self.dispatcher_mutation(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version FROM outbox WHERE state IN ('pending','uncertain') ORDER BY intent_id LIMIT ?1",
                )
                .map_err(sqlite_failure)?;
            let rows = stmt
                .query_map([limit], read_outbox)
                .map_err(sqlite_failure)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_failure)?;
            drop(stmt);
            claimed.clear();
            for row in rows {
                let next_attempt = row.attempt.checked_add(1).ok_or("attempt overflow")?;
                let next_version = row.version.checked_add(1).ok_or("version overflow")?;
                let n = conn
                    .execute(
                        "UPDATE outbox SET state='dispatching',attempt=?1,version=?2 WHERE intent_id=?3 AND version=?4 AND state IN ('pending','uncertain')",
                        rusqlite::params![
                            next_attempt as i64,
                            next_version as i64,
                            &row.intent_id,
                            row.version as i64
                        ],
                    )
                    .map_err(sqlite_failure)?;
                if n != 1 {
                    return Err("outbox claim conflict".into());
                }
                claimed.push(OutboxRecord {
                    state: OutboxState::Dispatching.as_str().into(),
                    attempt: next_attempt,
                    version: next_version,
                    ..row
                });
            }
            Ok(())
        })?;
        Ok(claimed)
    }

    /// Mark in-flight (`dispatching`) rows uncertain after a host crash.
    pub fn reconcile_inflight_outbox(&mut self) -> Result<u64> {
        let mut changed = 0u64;
        self.dispatcher_mutation(|conn| {
            changed = conn
                .execute(
                    "UPDATE outbox SET state='uncertain',version=version+1 WHERE state='dispatching'",
                    [],
                )
                .map_err(sqlite_failure)? as u64;
            Ok(())
        })?;
        Ok(changed)
    }

    /// Record a terminal or uncertain outcome for a claimed row. CAS on version.
    pub fn record_outbox_state(
        &mut self,
        intent_id: &str,
        version: u64,
        state: OutboxState,
    ) -> Result<u64> {
        if matches!(state, OutboxState::Pending | OutboxState::Dispatching) {
            return Err("invalid outbox outcome".into());
        }
        if intent_id.is_empty() || intent_id.len() > 128 {
            return Err("invalid intent id".into());
        }
        let next = version.checked_add(1).ok_or("version overflow")?;
        self.dispatcher_mutation(|conn| {
            let n = conn
                .execute(
                    "UPDATE outbox SET state=?1,version=?2 WHERE intent_id=?3 AND version=?4 AND state='dispatching'",
                    rusqlite::params![state.as_str(), next as i64, intent_id, version as i64],
                )
                .map_err(sqlite_failure)?;
            if n != 1 {
                return Err("outbox outcome conflict".into());
            }
            Ok(())
        })?;
        Ok(next)
    }

    pub fn get_outbox(&self, intent_id: &str) -> Result<Option<OutboxRecord>> {
        self.connection
            .query_row(
                "SELECT intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version FROM outbox WHERE intent_id=?1",
                [intent_id],
                read_outbox,
            )
            .optional()
            .map_err(sqlite_failure)
    }

    /// Insert or CAS-update a work_queue row. First write is insert.
    pub fn put_work_queue(&mut self, row: &WorkQueueRow) -> Result<u64> {
        if row.task_id.is_empty() || row.task_id.len() > 128 {
            return Err("invalid work_queue task".into());
        }
        let mut stored_version = row.version;
        self.dispatcher_mutation(|conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT version FROM work_queue WHERE task_id=?1",
                    [&row.task_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sqlite_failure)?;
            match existing {
                None => {
                    if row.version != 1 {
                        return Err("work_queue insert version".into());
                    }
                    conn.execute(
                        "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                        rusqlite::params![
                            &row.task_id,
                            &row.state,
                            row.priority,
                            &row.lease_owner,
                            row.lease_until,
                            row.attempt as i64,
                            row.version as i64
                        ],
                    )
                    .map_err(sqlite_failure)?;
                    stored_version = 1;
                }
                Some(version) if version as u64 == row.version => {
                    let next = row.version.checked_add(1).ok_or("version overflow")?;
                    let n = conn
                        .execute(
                            "UPDATE work_queue SET state=?1,priority=?2,lease_owner=?3,lease_until=?4,attempt=?5,version=?6 WHERE task_id=?7 AND version=?8",
                            rusqlite::params![
                                &row.state,
                                row.priority,
                                &row.lease_owner,
                                row.lease_until,
                                row.attempt as i64,
                                next as i64,
                                &row.task_id,
                                row.version as i64
                            ],
                        )
                        .map_err(sqlite_failure)?;
                    if n != 1 {
                        return Err("work_queue conflict".into());
                    }
                    stored_version = next;
                }
                _ => return Err("work_queue conflict".into()),
            }
            Ok(())
        })?;
        Ok(stored_version)
    }

    /// Requeue leased work whose lease_until has passed. Deterministic, no LLM.
    pub fn expire_work_leases(&mut self, now: i64) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        self.dispatcher_mutation(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT task_id FROM work_queue WHERE state='leased' AND lease_until<=?1 ORDER BY task_id",
                )
                .map_err(sqlite_failure)?;
            ids = stmt
                .query_map([now], |r| r.get(0))
                .map_err(sqlite_failure)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_failure)?;
            drop(stmt);
            for id in &ids {
                let n = conn
                    .execute(
                        "UPDATE work_queue SET state='pending',lease_owner=NULL,lease_until=NULL,attempt=attempt+1,version=version+1 WHERE task_id=?1 AND state='leased' AND lease_until<=?2",
                        rusqlite::params![id, now],
                    )
                    .map_err(sqlite_failure)?;
                if n != 1 {
                    return Err("work_queue lease conflict".into());
                }
            }
            Ok(())
        })?;
        Ok(ids)
    }

    pub fn get_work_queue(&self, task_id: &str) -> Result<Option<WorkQueueRow>> {
        self.connection
            .query_row(
                "SELECT task_id,state,priority,lease_owner,lease_until,attempt,version FROM work_queue WHERE task_id=?1",
                [task_id],
                |r| {
                    Ok(WorkQueueRow {
                        task_id: r.get(0)?,
                        state: r.get(1)?,
                        priority: r.get(2)?,
                        lease_owner: r.get(3)?,
                        lease_until: r.get(4)?,
                        attempt: r.get::<_, i64>(5)? as u64,
                        version: r.get::<_, i64>(6)? as u64,
                    })
                },
            )
            .optional()
            .map_err(sqlite_failure)
    }

    /// Dispatching intents that never recorded a remote binding. Those are the
    /// crash window before create/claim was durable; bound rows stay leased.
    pub fn unbound_dispatching(&self) -> Result<Vec<OutboxRecord>> {
        let rows = self.outbox_in_state(OutboxState::Dispatching.as_str())?;
        let mut unbound = Vec::new();
        for row in rows {
            if self.get_binding(&row.intent_id)?.is_none() {
                unbound.push(row);
            }
        }
        Ok(unbound)
    }

    pub fn outbox_in_state(&self, state: &str) -> Result<Vec<OutboxRecord>> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version FROM outbox WHERE state=?1 ORDER BY intent_id",
            )
            .map_err(sqlite_failure)?;
        stmt.query_map([state], read_outbox)
            .map_err(sqlite_failure)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_failure)
    }

    pub fn work_in_state(&self, state: &str) -> Result<Vec<WorkQueueRow>> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT task_id,state,priority,lease_owner,lease_until,attempt,version FROM work_queue WHERE state=?1 ORDER BY task_id",
            )
            .map_err(sqlite_failure)?;
        stmt.query_map([state], read_work)
            .map_err(sqlite_failure)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_failure)
    }

    /// Remote id recorded after a successful public-API create. Survives lease expiry.
    pub fn put_binding(&mut self, intent_id: &str, remote_id: &str, task_id: &str) -> Result<()> {
        if intent_id.is_empty() || remote_id.is_empty() || task_id.is_empty() {
            return Err("invalid dispatch binding".into());
        }
        let id = binding_id(intent_id);
        let data = encode_binding(remote_id, task_id, "bound", "");
        self.dispatcher_mutation(|conn| {
            let existing: Option<(i64, String)> = conn
                .query_row(
                    "SELECT version, data_json FROM objects WHERE id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sqlite_failure)?;
            match existing {
                None => {
                    conn.execute(
                        "INSERT INTO objects(id,type,status,data_json,version,created_at,updated_at) VALUES (?1,'dispatch_binding','active',?2,1,0,0)",
                        rusqlite::params![&id, &data],
                    )
                    .map_err(sqlite_failure)?;
                }
                Some((_, current)) => {
                    let parsed = decode_binding(&current);
                    if parsed.remote_id != remote_id || parsed.task_id != task_id {
                        return Err("binding conflict".into());
                    }
                    // Idempotent. Do not regress a claim phase recorded after create.
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn get_binding(&self, intent_id: &str) -> Result<Option<String>> {
        Ok(self.get_binding_record(intent_id)?.map(|row| row.remote_id))
    }

    pub fn get_binding_record(&self, intent_id: &str) -> Result<Option<DispatchBinding>> {
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT data_json FROM objects WHERE id=?1 AND type='dispatch_binding'",
                [binding_id(intent_id)],
                |r| r.get(0),
            )
            .optional()
            .map_err(sqlite_failure)?;
        Ok(data.map(|raw| decode_binding(&raw)))
    }

    /// Advance the crash marker. Never regresses `claimed` back to `claiming`.
    pub fn set_binding_phase(&mut self, intent_id: &str, phase: &str, token: &str) -> Result<()> {
        if !matches!(phase, "bound" | "claiming" | "claimed") {
            return Err("invalid binding phase".into());
        }
        if phase == "claimed" && token.is_empty() {
            return Err("claimed binding needs a token".into());
        }
        if token.contains('\t') || phase.contains('\t') {
            return Err("invalid binding token".into());
        }
        let id = binding_id(intent_id);
        self.dispatcher_mutation(|conn| {
            let existing: Option<(i64, String)> = conn
                .query_row(
                    "SELECT version, data_json FROM objects WHERE id=?1 AND type='dispatch_binding'",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sqlite_failure)?;
            let Some((version, raw)) = existing else {
                return Err("missing dispatch binding".into());
            };
            let current = decode_binding(&raw);
            if current.phase == "claimed" && phase != "claimed" {
                return Ok(());
            }
            if current.phase == phase && current.token == token {
                return Ok(());
            }
            let data = encode_binding(&current.remote_id, &current.task_id, phase, token);
            let n = conn
                .execute(
                    "UPDATE objects SET data_json=?1,version=version+1 WHERE id=?2 AND version=?3 AND type='dispatch_binding'",
                    rusqlite::params![&data, &id, version],
                )
                .map_err(sqlite_failure)?;
            if n != 1 {
                return Err("binding conflict".into());
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Terminal outbox transition, append-only event, and pending delivery in one fence.
    /// A matching lease is required when `lease` is `Some`. Replay of the same
    /// terminal state is a no-op so a crashed caller can read the delivery back.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_outcome(
        &mut self,
        intent_id: &str,
        version: u64,
        state: OutboxState,
        event_id: &str,
        task_id: &str,
        event_type: &str,
        payload_json: &str,
        created_at: i64,
        lease: Option<(&str, u64)>,
    ) -> Result<()> {
        if matches!(state, OutboxState::Pending | OutboxState::Dispatching) {
            return Err("invalid outbox outcome".into());
        }
        if event_id.is_empty() || task_id.is_empty() || payload_json.len() > 8192 {
            return Err("invalid outcome event".into());
        }
        let delivery_id = delivery_id(event_id);
        let next = version.checked_add(1).ok_or("version overflow")?;
        self.dispatcher_mutation(|conn| {
            let current: Option<String> = conn
                .query_row(
                    "SELECT state FROM outbox WHERE intent_id=?1",
                    [intent_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sqlite_failure)?;
            if current.as_deref() == Some(state.as_str()) {
                return Ok(());
            }
            if let Some((token, attempt)) = lease {
                let matched: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM work_queue WHERE task_id=?1 AND state='leased' AND lease_owner=?2 AND attempt=?3",
                        rusqlite::params![task_id, token, attempt as i64],
                        |r| r.get(0),
                    )
                    .map_err(sqlite_failure)?;
                if matched != 1 {
                    return Err("stale attempt".into());
                }
            }
            let n = conn
                .execute(
                    "UPDATE outbox SET state=?1,version=?2 WHERE intent_id=?3 AND version=?4 AND state='dispatching'",
                    rusqlite::params![state.as_str(), next as i64, intent_id, version as i64],
                )
                .map_err(sqlite_failure)?;
            if n != 1 {
                return Err("outbox outcome conflict".into());
            }
            conn.execute(
                "INSERT INTO events(id, object_id, event_type, payload_json, created_at, version) VALUES (?1,?2,?3,?4,?5,1)",
                rusqlite::params![event_id, task_id, event_type, payload_json, created_at],
            )
            .map_err(sqlite_failure)?;
            conn.execute(
                "INSERT INTO objects(id,type,status,data_json,version,created_at,updated_at) VALUES (?1,'delivery','pending',?2,1,?3,?3)",
                rusqlite::params![&delivery_id, payload_json, created_at],
            )
            .map_err(sqlite_failure)?;
            if let Some((token, attempt)) = lease {
                let n = conn
                    .execute(
                        "UPDATE work_queue SET state='done',lease_until=NULL,version=version+1 WHERE task_id=?1 AND state='leased' AND lease_owner=?2 AND attempt=?3",
                        rusqlite::params![task_id, token, attempt as i64],
                    )
                    .map_err(sqlite_failure)?;
                if n != 1 {
                    return Err("work_queue outcome conflict".into());
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn pending_deliveries(&self) -> Result<Vec<PendingDelivery>> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT id, data_json FROM objects WHERE type='delivery' AND status='pending' ORDER BY id",
            )
            .map_err(sqlite_failure)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(sqlite_failure)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_failure)?;
        Ok(rows
            .into_iter()
            .map(|(id, payload_json)| PendingDelivery {
                event_id: id.strip_prefix("delivery:").unwrap_or(&id).to_owned(),
                payload_json,
            })
            .collect())
    }

    pub fn acknowledge_delivery(&mut self, event_id: &str) -> Result<()> {
        let id = delivery_id(event_id);
        self.dispatcher_mutation(|conn| {
            let n = conn
                .execute(
                    "UPDATE objects SET status='acknowledged',updated_at=updated_at,version=version+1 WHERE id=?1 AND type='delivery' AND status='pending'",
                    [&id],
                )
                .map_err(sqlite_failure)?;
            if n != 1 {
                return Err("delivery not pending".into());
            }
            Ok(())
        })?;
        Ok(())
    }
}

fn binding_id(intent_id: &str) -> String {
    format!("bind:{intent_id}")
}
fn encode_binding(remote_id: &str, task_id: &str, phase: &str, token: &str) -> String {
    format!("{remote_id}\t{task_id}\t{phase}\t{token}")
}
fn decode_binding(raw: &str) -> DispatchBinding {
    let mut parts = raw.splitn(4, '\t');
    let remote_id = parts.next().unwrap_or("").to_owned();
    let task_id = parts.next().unwrap_or("").to_owned();
    let phase = parts
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("bound");
    let token = parts.next().unwrap_or("").to_owned();
    DispatchBinding {
        remote_id,
        task_id,
        phase: phase.to_owned(),
        token,
    }
}
fn delivery_id(event_id: &str) -> String {
    format!("delivery:{event_id}")
}
fn read_work(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkQueueRow> {
    Ok(WorkQueueRow {
        task_id: r.get(0)?,
        state: r.get(1)?,
        priority: r.get(2)?,
        lease_owner: r.get(3)?,
        lease_until: r.get(4)?,
        attempt: r.get::<_, i64>(5)? as u64,
        version: r.get::<_, i64>(6)? as u64,
    })
}
fn read_outbox(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRecord> {
    Ok(OutboxRecord {
        intent_id: r.get(0)?,
        idempotency_key: r.get(1)?,
        kind: r.get(2)?,
        payload_cbor_hex: r.get(3)?,
        correlation_id: r.get(4)?,
        state: r.get(5)?,
        attempt: r.get::<_, i64>(6)? as u64,
        version: r.get::<_, i64>(7)? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "coglet:1111111111111111111111111111111111111111111111111111111111111111";
    const SCHEMA: &str = "
CREATE TABLE objects (id TEXT PRIMARY KEY NOT NULL);
INSERT INTO objects VALUES('task-1');
CREATE TABLE work_queue (
    task_id TEXT PRIMARY KEY NOT NULL REFERENCES objects(id),
    state TEXT NOT NULL,
    priority BIGINT NOT NULL,
    lease_owner TEXT,
    lease_until BIGINT,
    attempt BIGINT NOT NULL CHECK (attempt >= 0),
    version BIGINT NOT NULL CHECK (version > 0)
);
CREATE TABLE outbox (
    intent_id TEXT PRIMARY KEY NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    payload_cbor_hex TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','dispatching','uncertain','succeeded','failed','cancelled')),
    attempt BIGINT NOT NULL CHECK (attempt >= 0),
    version BIGINT NOT NULL CHECK (version > 0)
);";

    fn open() -> (tempfile::TempDir, Database) {
        let root = tempfile::tempdir().unwrap();
        let mut db = Database::open(root.path(), OWNER).unwrap();
        db.provision_for_test(SCHEMA).unwrap();
        db.connection
            .execute(
                "INSERT INTO outbox VALUES('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','factory.dispatch','00','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','pending',0,1)",
                [],
            )
            .unwrap();
        (root, db)
    }

    #[test]
    fn claim_retry_keeps_idempotency_key() {
        let (_root, mut db) = open();
        let first = db.claim_pending_outbox(8).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].idempotency_key, first[0].intent_id);
        assert_eq!(first[0].attempt, 1);
        db.record_outbox_state(
            &first[0].intent_id,
            first[0].version,
            OutboxState::Uncertain,
        )
        .unwrap();
        let second = db.claim_pending_outbox(8).unwrap();
        assert_eq!(second[0].intent_id, first[0].intent_id);
        assert_eq!(second[0].idempotency_key, first[0].idempotency_key);
        assert_eq!(second[0].attempt, 2);
        assert_eq!(
            db.get_outbox(&first[0].intent_id).unwrap().unwrap().state,
            "dispatching"
        );
    }

    #[test]
    fn inflight_reconcile_then_reclaim() {
        let (_root, mut db) = open();
        let claimed = db.claim_pending_outbox(1).unwrap();
        assert_eq!(db.reconcile_inflight_outbox().unwrap(), 1);
        assert_eq!(
            db.get_outbox(&claimed[0].intent_id).unwrap().unwrap().state,
            "uncertain"
        );
        let again = db.claim_pending_outbox(1).unwrap();
        assert_eq!(again[0].idempotency_key, claimed[0].idempotency_key);
    }

    #[test]
    fn lease_expiration_requeues_work_queue() {
        let (_root, mut db) = open();
        db.put_work_queue(&WorkQueueRow {
            task_id: "task-1".into(),
            state: "leased".into(),
            priority: 1,
            lease_owner: Some("worker".into()),
            lease_until: Some(10),
            attempt: 1,
            version: 1,
        })
        .unwrap();
        assert!(db.expire_work_leases(9).unwrap().is_empty());
        assert_eq!(
            db.expire_work_leases(10).unwrap(),
            vec!["task-1".to_string()]
        );
        let row = db.get_work_queue("task-1").unwrap().unwrap();
        assert_eq!(row.state, "pending");
        assert!(row.lease_owner.is_none());
        assert_eq!(row.attempt, 2);
    }

    #[test]
    fn duplicate_claim_is_empty() {
        let (_root, mut db) = open();
        assert_eq!(db.claim_pending_outbox(1).unwrap().len(), 1);
        assert!(db.claim_pending_outbox(1).unwrap().is_empty());
    }
}
