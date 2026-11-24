//! Native admission only; this does not enable the runtime DB import.
use super::*;
use coglet_types::Value as V;
use rusqlite::OptionalExtension;
use std::{collections::BTreeSet, io::Read};

/// Trusted deployment pins. These are host configuration, never guest input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pins {
    pub code_hash: String,
    pub schema_version: u64,
    pub schema_hash: String,
    pub catalog_hash: String,
    pub grants: Grants,
    pub intent_kinds: BTreeSet<String>,
}
pub struct Admission {
    pub pins: Pins,
    pub schema_min: u64,
    pub schema_max: u64,
    pub deadline: Instant,
}

pub(super) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS _coglet_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1), revision BLOB NOT NULL CHECK(length(revision)=8), attempts BLOB NOT NULL CHECK(length(attempts)=8));
    INSERT OR IGNORE INTO _coglet_state VALUES(1,zeroblob(8),zeroblob(8));
    CREATE TABLE IF NOT EXISTS _coglet_cursors(token_hash TEXT PRIMARY KEY NOT NULL, binding TEXT NOT NULL, attempt BLOB NOT NULL, position BLOB NOT NULL, last_key TEXT NOT NULL);")
        .map_err(|e| e.to_string())
}
fn number(bytes: Vec<u8>) -> Result<u64> {
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| "corrupt host counter")?,
    ))
}
fn digest(value: V) -> String {
    // Host-only finite values built here always satisfy the profile.
    hex::encode(Sha256::digest(
        coglet_types::encode_profile(&value).expect("host profile"),
    ))
}
fn cells(row: &[Cell]) -> V {
    V::Array(
        row.iter()
            .map(|c| match c {
                Cell::Null => V::Null,
                Cell::Integer(n) => V::Integer((*n).into()),
                Cell::Unsigned(n) => V::Integer((*n).into()),
                Cell::Text(s) => V::Text(s.clone()),
            })
            .collect(),
    )
}
pub(super) fn row_key(row: &[Cell]) -> String {
    digest(cells(row))
}
impl Database {
    /// Exact live schema fingerprint, excluding only host-owned metadata.
    pub fn schema_pin(&self) -> Result<(u64, String)> {
        let version: u64 = self
            .connection
            .query_row("PRAGMA user_version", [], |r| {
                r.get::<_, u32>(0).map(u64::from)
            })
            .map_err(|e| e.to_string())?;
        let mut stmt = self.connection.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE '_coglet_%' ORDER BY type,name").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(V::Array(vec![
                    V::Text(r.get(0)?),
                    V::Text(r.get(1)?),
                    V::Text(r.get(2)?),
                    r.get::<_, Option<String>>(3)?.map_or(V::Null, V::Text),
                ]))
            })
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok((version, digest(V::Array(rows))))
    }
    pub fn counters(&self) -> Result<(u64, u64)> {
        let (revision, attempts) = self
            .connection
            .query_row(
                "SELECT revision,attempts FROM _coglet_state WHERE singleton=1",
                [],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
            )
            .map_err(|e| e.to_string())?;
        Ok((number(revision)?, number(attempts)?))
    }
    /// Authenticate the executing code separately from the approved deployment.
    /// Own the grant snapshot. Failed admission never enters/counts a guest step;
    /// every admitted attempt is durably counted even when it makes no DB calls.
    pub fn admit<'a>(
        &'a mut self,
        catalog: &'a Catalog,
        executing_code_hash: &str,
        admission: Admission,
    ) -> Result<Unit<'a>> {
        let p = admission.pins;
        let valid_hash = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        if !valid_hash(&p.code_hash)
            || p.code_hash != executing_code_hash
            || p.catalog_hash != catalog.digest()?
            || (p.schema_version, p.schema_hash.clone()) != self.schema_pin()?
            || admission.schema_min > admission.schema_max
            || !(admission.schema_min..=admission.schema_max).contains(&p.schema_version)
        {
            return Err("admission pin mismatch".into());
        }
        if Instant::now() >= admission.deadline {
            return Err("deadline exceeded".into());
        }
        if !self.connection.is_autocommit() {
            return Err("unreconciled database unit".into());
        }
        let (revision, attempts) = self.counters()?;
        let attempt = attempts
            .checked_add(1)
            .ok_or("admission counter overflow")?;
        // SQLite autocommit with synchronous=FULL: not part of the later lazy unit.
        self.connection
            .execute(
                "UPDATE _coglet_state SET attempts=?1 WHERE singleton=1",
                [attempt.to_be_bytes().to_vec()],
            )
            .map_err(|e| e.to_string())?;
        // No cursor can survive 256 further admissions. Reclaim expired state
        // under the owner lease; fixed-width big-endian BLOB ordering is numeric.
        if attempt >= 256 {
            self.connection
                .execute(
                    "DELETE FROM _coglet_cursors WHERE attempt<=?1",
                    [(attempt - 256).to_be_bytes().to_vec()],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(Unit {
            db: self,
            catalog,
            grants: p.grants,
            deadline: admission.deadline,
            poisoned: false,
            calls: 0,
            intent_grants: Some(p.intent_kinds.clone()),
            finished: false,
            pins: Some(p),
            revision,
            attempt,
            wrote: false,
            paging: false,
            transaction: false,
        })
    }
}
impl Unit<'_> {
    pub(super) fn rollback_failed_unit(&mut self) {
        let _ = self.db.connection.progress_handler(0, None::<fn() -> bool>);
        let _ = self.db.connection.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        if self.transaction && self.db.connection.execute_batch("ROLLBACK").is_ok() {
            self.transaction = false;
        }
    }
    pub fn pins(&self) -> Option<&Pins> {
        self.pins.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Provisional revision for the future prepared-decision coordinator.
    /// Merely computing it cannot persist or commit an application unit.
    pub fn resulting_revision(&self) -> Result<u64> {
        if self.wrote {
            self.revision
                .checked_add(1)
                .ok_or("revision overflow".into())
        } else {
            Ok(self.revision)
        }
    }
    pub fn attempt(&self) -> u64 {
        self.attempt
    }
    pub(super) fn start_transaction(&mut self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err("resource exhausted".into());
        }
        if !self.transaction {
            let deadline = self.deadline;
            self.db
                .connection
                .progress_handler(1000, Some(move || Instant::now() >= deadline))
                .map_err(|e| e.to_string())?;
            self.db
                .connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            self.transaction = true;
        }
        Ok(())
    }
    fn cursor_binding(&self, id: &str, params: &[Cell]) -> Result<String> {
        let p = self.pins.as_ref().ok_or("cursor conflict")?;
        Ok(digest(V::Array(vec![
            V::Text(self.db.owner.clone()),
            V::Text(id.into()),
            cells(params),
            V::Text(p.catalog_hash.clone()),
            V::Text(p.schema_hash.clone()),
            V::Integer(p.schema_version.into()),
            V::Integer(self.revision.into()),
        ])))
    }
    pub(super) fn cursor_position(
        &self,
        id: &str,
        params: &[Cell],
        token: Option<&str>,
    ) -> Result<(u64, Option<String>)> {
        let Some(token) = token else {
            return Ok((0, None));
        };
        let hash = hex::encode(Sha256::digest(token.as_bytes()));
        let row: Option<(String, Vec<u8>, Vec<u8>, String)> = self
            .db
            .connection
            .query_row(
                "SELECT binding,attempt,position,last_key FROM _coglet_cursors WHERE token_hash=?1",
                [hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (binding, attempt, position, last_key) = row.ok_or("cursor conflict")?;
        let age = self
            .attempt
            .checked_sub(number(attempt)?)
            .ok_or("cursor conflict")?;
        if binding != self.cursor_binding(id, params)? || age >= 256 {
            return Err("cursor conflict".into());
        }
        Ok((number(position)?, Some(last_key)))
    }
    /// Stateful authenticated cursor: a 256-bit OS-random bearer token, only its
    /// SHA256 verifier stored beside immutable bindings. No client fields to forge.
    /// Records are private and transactionally retained with read commit markers.
    pub(super) fn mint_cursor(
        &self,
        id: &str,
        params: &[Cell],
        position: u64,
        row: &[Cell],
    ) -> Result<Option<String>> {
        let mut random = [0u8; 32];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(|e| e.to_string())?;
        let token = hex::encode(random);
        self.db.connection.execute("INSERT INTO _coglet_cursors(token_hash,binding,attempt,position,last_key) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![hex::encode(Sha256::digest(token.as_bytes())),self.cursor_binding(id,params)?,self.attempt.to_be_bytes().to_vec(),position.to_be_bytes().to_vec(),row_key(row)]).map_err(|e| e.to_string())?;
        Ok(Some(token))
    }
}
