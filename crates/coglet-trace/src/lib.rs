//! Local durable hub trace archive. No remote authorization is implied by this API.
use coglet_runtime::Runtime;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub archive_bytes: usize,
    pub total_bytes: u64,
    pub entries: u64,
    pub query_rows: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            archive_bytes: 16 * 1024 * 1024,
            total_bytes: 1024 * 1024 * 1024,
            entries: 100_000,
            query_rows: 1000,
        }
    }
}

/// Inclusive hub-observed Unix-millisecond interval, not runtime logical time.
pub struct Query<'a> {
    pub coglet: Option<&'a str>,
    pub run: Option<&'a str>,
    pub since: i64,
    pub until: i64,
    pub limit: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub coglet: String,
    pub run: String,
    pub observed_at: i64,
}
pub struct Store {
    db: Connection,
    limits: Limits,
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Store {
    /// The database and its parent must be supervisor-owned private paths.
    pub fn open(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS archives(id TEXT PRIMARY KEY, body BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS entries(coglet TEXT NOT NULL, run TEXT NOT NULL,
              observed_at INTEGER NOT NULL, id TEXT NOT NULL REFERENCES archives(id),
              PRIMARY KEY(coglet,run,observed_at,id));
            CREATE INDEX IF NOT EXISTS trace_time ON entries(observed_at,id);
            CREATE INDEX IF NOT EXISTS trace_run ON entries(run,observed_at,id);",
        )?;
        let policy = format!(
            "v1:{}:{}:{}:{}",
            limits.archive_bytes, limits.total_bytes, limits.entries, limits.query_rows
        );
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS policy(singleton INTEGER PRIMARY KEY CHECK(singleton=1), value TEXT NOT NULL)")?;
        tx.execute("INSERT OR IGNORE INTO policy VALUES(1,?1)", [&policy])?;
        let retained: String =
            tx.query_row("SELECT value FROM policy WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        if retained != policy {
            return Err("storage policy mismatch".into());
        }
        tx.commit()?;
        Ok(Self { db, limits })
    }

    /// Caller authenticates source and authorizes all private archive contents.
    /// Run identity and observed time are trusted hub metadata, never producer claims.
    pub fn ingest(&mut self, run: &str, observed_at: i64, bytes: &[u8]) -> Result<String> {
        if bytes.len() > self.limits.archive_bytes {
            return Err("archive byte limit".into());
        }
        if run.is_empty()
            || run.len() > 128
            || !run
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            || observed_at < 0
        {
            return Err("invalid trusted run/time metadata".into());
        }
        let value: Value = serde_json::from_slice(bytes)?;
        Runtime::import_trace(&value).map_err(|e| format!("invalid archive: {e}"))?;
        let coglet = value["coglet"].as_str().ok_or("missing coglet")?;
        let id = hash(bytes);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO archives VALUES(?1,?2)",
            params![id, bytes],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO entries VALUES(?1,?2,?3,?4)",
            params![coglet, run, observed_at, id],
        )?;
        let used: i64 = tx.query_row(
            "SELECT coalesce(sum(length(body)),0) FROM archives",
            [],
            |r| r.get(0),
        )?;
        let entries: i64 = tx.query_row("SELECT count(*) FROM entries", [], |r| r.get(0))?;
        if u64::try_from(used)? > self.limits.total_bytes
            || u64::try_from(entries)? > self.limits.entries
        {
            return Err("store quota exceeded".into());
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn query(&self, q: Query<'_>) -> Result<Vec<Entry>> {
        if q.limit == 0
            || q.limit > self.limits.query_rows
            || q.limit > i64::MAX as usize
            || q.since < 0
            || q.until < q.since
        {
            return Err("invalid query bounds".into());
        }
        let mut stmt = self.db.prepare(
            "SELECT id,coglet,run,observed_at FROM entries
            WHERE (?1 IS NULL OR coglet=?1) AND (?2 IS NULL OR run=?2)
            AND observed_at BETWEEN ?3 AND ?4 ORDER BY observed_at,id,coglet,run LIMIT ?5",
        )?;
        let rows = stmt.query_map(
            params![q.coglet, q.run, q.since, q.until, q.limit as i64],
            |r| {
                Ok(Entry {
                    id: r.get(0)?,
                    coglet: r.get(1)?,
                    run: r.get(2)?,
                    observed_at: r.get(3)?,
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Only expose bytes after caller authorizes archive export to its principal.
    pub fn fetch(&self, id: &str) -> Result<Vec<u8>> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid archive id".into());
        }
        let bytes: Vec<u8> = self
            .db
            .query_row(
                "SELECT body FROM archives WHERE id=?1 AND length(body)<=?2",
                params![id, i64::try_from(self.limits.archive_bytes)?],
                |r| r.get(0),
            )
            .optional()?
            .ok_or("archive missing or oversized")?;
        if hash(&bytes) != id {
            return Err("stored archive digest mismatch".into());
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        Runtime::import_trace(&value).map_err(|e| format!("stored archive invalid: {e}"))?;
        Ok(bytes)
    }
}
