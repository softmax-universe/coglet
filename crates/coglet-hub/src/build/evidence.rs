//! Digest-addressed evidence in the existing Store and artifact transaction.
use super::{Error, Result, Store, checked, hash};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;

const MAX_BLOB: usize = 64 * 1024 * 1024;
const MAX_TOTAL: i64 = 256 * 1024 * 1024;
const MAX_COUNT: i64 = 1024;

pub(super) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS build_evidence(sha256 TEXT PRIMARY KEY NOT NULL,bytes BLOB NOT NULL CHECK(length(bytes) BETWEEN 1 AND 67108864));
    CREATE TRIGGER IF NOT EXISTS build_evidence_no_replace BEFORE INSERT ON build_evidence WHEN EXISTS(SELECT 1 FROM build_evidence WHERE sha256=NEW.sha256) BEGIN SELECT RAISE(ABORT,'retained evidence'); END;
    CREATE TRIGGER IF NOT EXISTS build_evidence_no_update BEFORE UPDATE ON build_evidence BEGIN SELECT RAISE(ABORT,'immutable evidence'); END;
    CREATE TRIGGER IF NOT EXISTS build_evidence_no_delete BEFORE DELETE ON build_evidence BEGIN SELECT RAISE(ABORT,'retained evidence'); END;")?;
    Ok(())
}

impl Store {
    /// Trusted-local custody read, not remote export or renewed effect authority.
    pub fn build_evidence(&self, digest: &str) -> Result<Option<Vec<u8>>> {
        checked("hub-hash", &json!(digest))?;
        let bytes: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT bytes FROM build_evidence WHERE sha256=?1",
                [digest],
                |r| r.get(0),
            )
            .optional()?;
        if bytes
            .as_ref()
            .is_some_and(|b| b.is_empty() || b.len() > MAX_BLOB || hash(b) != digest)
        {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }
}

pub(super) fn retain(tx: &Transaction<'_>, blobs: &[Vec<u8>]) -> Result<()> {
    let (mut count, mut total): (i64, i64) = tx.query_row(
        "SELECT count(*),coalesce(sum(length(bytes)),0) FROM build_evidence",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    for bytes in blobs {
        if bytes.is_empty() || bytes.len() > MAX_BLOB {
            return Err(Error::Full);
        }
        let digest = hash(bytes);
        let existing: Option<Vec<u8>> = tx
            .query_row(
                "SELECT bytes FROM build_evidence WHERE sha256=?1",
                [&digest],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = existing {
            if old != *bytes {
                return Err(Error::Invalid);
            }
            continue;
        }
        if count < 0 || total < 0 || count >= MAX_COUNT || total + bytes.len() as i64 > MAX_TOTAL {
            return Err(Error::Full);
        }
        tx.execute(
            "INSERT INTO build_evidence(sha256,bytes) VALUES(?1,?2)",
            params![digest, bytes],
        )?;
        count += 1;
        total += bytes.len() as i64;
    }
    Ok(())
}
