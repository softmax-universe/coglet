//! Fixed hand-edited hub kernel. No product code or guest-controlled policy.
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub mod build;
pub mod kernel;
pub mod operator;
pub use kernel::{ActionGrant, EventGrant, Kernel, Registration};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("conflict (expected offset {0})")]
    Conflict(u64),
    #[error("invalid_argument")]
    Invalid,
    #[error("resource_exhausted")]
    Full,
    #[error("permission_denied")]
    Denied,
    #[error("split authority.db state requires explicit offline migration")]
    SplitAuthority,
    #[error("{0}")]
    Authority(#[from] coglet_proto::authority::AuthorityError),
    #[error("storage unavailable")]
    Storage(#[from] rusqlite::Error),
    #[error("invalid stored encoding")]
    Encoding(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub struct Store {
    db: Connection,
}

fn number(s: String) -> Result<u64> {
    s.parse().map_err(|_| Error::Invalid)
}
fn audit(tx: &Transaction<'_>, record: Value) -> Result<()> {
    tx.execute(
        "INSERT INTO audit(record) VALUES(?1)",
        [serde_json::to_string(&record)?],
    )?;
    Ok(())
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS cursors(peer TEXT, target TEXT, next TEXT NOT NULL, PRIMARY KEY(peer,target));
          CREATE TABLE IF NOT EXISTS batches(peer TEXT, target TEXT, offset TEXT, input BLOB NOT NULL, next TEXT NOT NULL, PRIMARY KEY(peer,target,offset));
          CREATE TABLE IF NOT EXISTS watermarks(target TEXT, source TEXT, seq TEXT NOT NULL, PRIMARY KEY(target,source));
          CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY AUTOINCREMENT, target TEXT NOT NULL, event TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS outcomes(target TEXT, action_id TEXT, event TEXT NOT NULL, PRIMARY KEY(target,action_id));
          CREATE TABLE IF NOT EXISTS actions(target TEXT, id TEXT, peer TEXT NOT NULL, request TEXT NOT NULL, PRIMARY KEY(target,id));
          CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY, record TEXT NOT NULL);
          CREATE TRIGGER IF NOT EXISTS audit_no_update BEFORE UPDATE ON audit BEGIN SELECT RAISE(ABORT,'append only'); END;
          CREATE TRIGGER IF NOT EXISTS audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(ABORT,'append only'); END;")?;
        kernel::promotion::initialize(&db)?;
        operator::initialize(&db)?;
        build::initialize(&db)?;
        Ok(Self { db })
    }
    /// Caller authorizes the authenticated relay binding before this atomic admission.
    /// Event bytes use the same deterministic profile as the transport, not JSON ordering.
    pub fn admit_events(
        &mut self,
        peer: &str,
        target: &str,
        offset: u64,
        events: &[Value],
    ) -> Result<u64> {
        if events.len() > 64 {
            return Err(Error::Invalid);
        }
        let encoded = coglet_proto::wire::encode(&json!(events)).map_err(|_| Error::Invalid)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let expected = tx
            .query_row(
                "SELECT next FROM cursors WHERE peer=?1 AND target=?2",
                params![peer, target],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(number)
            .transpose()?
            .unwrap_or(0);
        if !events.is_empty() {
            let old = tx
                .query_row(
                    "SELECT input,next FROM batches WHERE peer=?1 AND target=?2 AND offset=?3",
                    params![peer, target, offset.to_string()],
                    |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?;
            if let Some((input, next)) = old {
                return if input == encoded {
                    number(next)
                } else {
                    Err(Error::Conflict(expected))
                };
            }
        }
        if offset != expected {
            return Err(Error::Conflict(expected));
        }
        if events.is_empty() {
            return Ok(expected);
        }
        let next = offset.checked_add(events.len() as u64).ok_or(Error::Full)?;
        let pending: i64 = tx.query_row(
            "SELECT count(*) FROM events WHERE target=?1",
            [target],
            |r| r.get(0),
        )?;
        if pending + events.len() as i64 > 256 {
            return Err(Error::Full);
        }
        for event in events {
            if event["kind"] == "action.outcome" {
                let action_id = event["payload"]["id"].as_str().ok_or(Error::Invalid)?;
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM outcomes WHERE target=?1 AND action_id=?2)",
                    params![target, action_id],
                    |r| r.get(0),
                )?;
                if exists {
                    return Err(Error::Conflict(expected));
                }
                tx.execute(
                    "INSERT INTO outcomes VALUES(?1,?2,?3)",
                    params![target, action_id, serde_json::to_string(event)?],
                )?;
            }
            let source = event["source"].as_str().ok_or(Error::Invalid)?;
            let seq = event["seq"].as_u64().ok_or(Error::Invalid)?;
            let watermark = tx
                .query_row(
                    "SELECT seq FROM watermarks WHERE target=?1 AND source=?2",
                    params![target, source],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(number)
                .transpose()?;
            if watermark.is_some_and(|old| seq <= old) {
                return Err(Error::Conflict(expected));
            }
            tx.execute("INSERT INTO watermarks VALUES(?1,?2,?3) ON CONFLICT(target,source) DO UPDATE SET seq=excluded.seq", params![target,source,seq.to_string()])?;
            tx.execute(
                "INSERT INTO events(target,event) VALUES(?1,?2)",
                params![target, serde_json::to_string(event)?],
            )?;
        }
        tx.execute(
            "INSERT INTO batches VALUES(?1,?2,?3,?4,?5)",
            params![peer, target, offset.to_string(), encoded, next.to_string()],
        )?;
        tx.execute("INSERT INTO cursors VALUES(?1,?2,?3) ON CONFLICT(peer,target) DO UPDATE SET next=excluded.next", params![peer,target,next.to_string()])?;
        audit(
            &tx,
            json!({"method":"hub.events","peer":peer,"target":target,"offset":offset,"next_offset":next,"outcome":"admitted"}),
        )?;
        tx.commit()?;
        Ok(next)
    }
    /// Stable local delivery tokens; never reused after acknowledgement/restart.
    pub fn event_deliveries(&self, target: &str) -> Result<Vec<(i64, Value)>> {
        let mut q = self
            .db
            .prepare("SELECT id,event FROM events WHERE target=?1 ORDER BY id")?;
        q.query_map([target], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (id, event) = row?;
            Ok((id, serde_json::from_str(&event)?))
        })
        .collect()
    }
    /// Local adapter calls only after its consumer durably commits processing.
    /// Batch receipts and source watermarks outlive payload consumption.
    pub fn ack_events(&mut self, target: &str, through: i64) -> Result<()> {
        if through < 1 {
            return Err(Error::Invalid);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count = tx.execute(
            "DELETE FROM events WHERE target=?1 AND id<=?2",
            params![target, through],
        )?;
        if count > 0 {
            audit(
                &tx,
                json!({"method":"kernel.events.ack","target":target,"through":through,"count":count,"outcome":"consumed"}),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn pending_events(&self, target: &str) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT event FROM events WHERE target=?1 ORDER BY id")?;
        q.query_map([target], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    /// Persist the once-mapped complete request before any network dispatch.
    pub fn retain_action(
        &mut self,
        peer: &str,
        target: &str,
        id: &str,
        request: &Value,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = tx
            .query_row(
                "SELECT peer,request FROM actions WHERE target=?1 AND id=?2",
                params![target, id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((old_peer, old)) = previous {
            return if old_peer == peer && serde_json::from_str::<Value>(&old)? == *request {
                Ok(())
            } else {
                Err(Error::Conflict(0))
            };
        }
        tx.execute(
            "INSERT INTO actions VALUES(?1,?2,?3,?4)",
            params![target, id, peer, serde_json::to_string(request)?],
        )?;
        audit(
            &tx,
            json!({"method":"hub.actions","target":target,"operation_id":request["correlation_id"],"action_id":id,"peer":peer,"outcome":"retained"}),
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn action(&self, target: &str, id: &str) -> Result<Option<(String, Value)>> {
        let row = self
            .db
            .query_row(
                "SELECT peer,request FROM actions WHERE target=?1 AND id=?2",
                params![target, id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        row.map(|(peer, request)| Ok((peer, serde_json::from_str(&request)?)))
            .transpose()
    }
    pub fn audit(&self) -> Result<Vec<Value>> {
        let mut q = self.db.prepare("SELECT record FROM audit ORDER BY id")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
}
