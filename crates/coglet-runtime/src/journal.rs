use crate::phase_evidence::{Phase, mark};
use crate::{Result, cbor, digest, profile_value};
use coglet_types::Value as V;
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const MAX_TRANSACTION: usize = 64 * 1024 * 1024;
#[cfg(test)]
#[path = "phase_wal_tests.rs"]
mod phase_tests;

/// Internal WAL, NOT the exported ABI journal. One fsynced frame atomically
/// holds all runtime host state and an optional ABI record. The ABI stream is
/// exported separately. An exclusive file lock prevents concurrent writers.
/// The supervisor must provide a private persistent parent directory.
///
/// Two frame kinds exist. The 0.1 frame (`state`, `journal`, `previous`,
/// `hash`) is unchanged and its record is JSON. A 0.2 database frame adds a
/// field `db_record`: the lowercase hex of the EXACT deterministic-CBOR
/// `db-step-record` bytes (the profile forbids CBOR byte strings, hex is the
/// repository's byte convention). Those bytes are authoritative and are never
/// re-encoded, so the ABI hash chain and the export are byte-exact whatever a
/// JSON view would do. On disk the frame's `journal` field is null; the derived
/// JSON projection (`database_projection`, lossless for every profile value
/// incl. uint64) exists only in memory. It is recomputed from the raw bytes on
/// every open, is used by the shared chain/segment/host-state checks, and must
/// never be exported or re-hashed.
/// Committed collaborator publications also carry `db_artifacts_hash`, the
/// SHA-256 receipt of exact prepared artifact bytes (not the derived state).
/// It participates in the WAL checksum/chain, never the ABI record or export.
/// Older frames and direct/aborted database writes may omit this receipt.
pub struct DurableStore {
    phase: Option<crate::phase_evidence::Sink>,
    #[cfg(test)]
    pub(crate) before_commit: Option<Box<dyn FnMut() + Send>>,
    file: LockedFile,
    state: Option<Value>,
    journal: Vec<Value>,
    raw: Vec<Option<Vec<u8>>>,
    db_artifacts_hashes: Vec<Option<String>>,
    previous: Value,
    poisoned: bool,
}

/// The exclusively locked WAL file. The lock is released explicitly on drop
/// instead of relying on `close`: it belongs to the open file description,
/// which a concurrently spawned child process shares until its `exec` closes
/// the inherited descriptor. Closing alone would leave the WAL "already owned"
/// for that window, so the next open in this process could be refused. Every
/// path that drops the file, including a failed recovery in `open`, unlocks.
struct LockedFile(File);

impl Drop for LockedFile {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl std::ops::Deref for LockedFile {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}

impl std::ops::DerefMut for LockedFile {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.0
    }
}

impl DurableStore {
    pub(crate) fn phase_sink(&self) -> Option<crate::phase_evidence::Sink> {
        self.phase.clone()
    }
    pub(crate) fn enable_phase_sink(&mut self) {
        self.phase
            .get_or_insert_with(crate::phase_evidence::Recorder::new);
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path).map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|e| format!("WAL already owned: {e}"))?;
        let mut file = LockedFile(file);
        // Also persist a newly created directory entry before admitting work.
        file.sync_all().map_err(|e| e.to_string())?;
        File::open(path.parent().ok_or("WAL needs a parent directory")?)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        let mut state = None;
        let mut journal = Vec::new();
        let mut raw_records: Vec<Option<Vec<u8>>> = Vec::new();
        let mut db_artifacts_hashes = Vec::new();
        let mut previous = Value::Null;
        let mut complete = 0;
        loop {
            let mut header = [0u8; 4];
            // read_exact doesn't distinguish clean EOF from partial header; both
            // recover to the last complete commit, never a guessed transaction.
            match file.read_exact(&mut header) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.to_string()),
            }
            let n = u32::from_be_bytes(header) as usize;
            if n == 0 || n > MAX_TRANSACTION {
                return Err("invalid WAL transaction length".into());
            }
            let mut raw = vec![0; n];
            match file.read_exact(&mut raw) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.to_string()),
            }
            let decoded = coglet_types::decode_profile(&raw)?;
            let V::Map(mut entries) = decoded else {
                return Err("WAL transaction is not a map".into());
            };
            let is_text = |k: &V, name: &str| matches!(k, V::Text(t) if t == name);
            let db_record = match entries.iter().position(|(k, _)| is_text(k, "db_record")) {
                Some(i) => match entries.remove(i).1 {
                    V::Text(hex_text) => Some(hex_text),
                    _ => return Err("WAL database record is not hex text".into()),
                },
                None => None,
            };
            let db_artifacts_hash = match entries
                .iter()
                .position(|(k, _)| is_text(k, "db_artifacts_hash"))
            {
                Some(i) => match entries.remove(i).1 {
                    V::Text(hash)
                        if hash.len() == 64
                            && hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
                    {
                        Some(hash)
                    }
                    _ => return Err("invalid WAL database artifacts hash".into()),
                },
                None => None,
            };
            if db_artifacts_hash.is_some() && db_record.is_none() {
                return Err("WAL artifacts receipt without database record".into());
            }
            let mut transaction: Value = V::Map(entries.clone())
                .deserialized()
                .map_err(|e| e.to_string())?;
            let object = transaction
                .as_object_mut()
                .ok_or("WAL transaction is not a map")?;
            if object.len() != 4 {
                return Err("unexpected WAL fields".into());
            }
            let hash = object.remove("hash").ok_or("missing WAL checksum")?;
            let expected = if let Some(hex_text) = &db_record {
                let mut unhashed = entries;
                unhashed.retain(|(k, _)| !is_text(k, "hash"));
                unhashed.push((V::Text("db_record".into()), V::Text(hex_text.clone())));
                if let Some(hash) = &db_artifacts_hash {
                    unhashed.push((V::Text("db_artifacts_hash".into()), V::Text(hash.clone())));
                }
                digest(&coglet_types::encode_profile(&V::Map(unhashed))?)
            } else {
                digest(&cbor(&transaction)?)
            };
            if transaction.get("previous") != Some(&previous) || hash != Value::String(expected) {
                return Err("WAL checksum/chain mismatch".into());
            }
            let record = transaction.get("journal").ok_or("missing WAL journal")?;
            if let Some(hex_text) = db_record {
                let bytes = hex::decode(&hex_text).map_err(|e| e.to_string())?;
                if hex::encode(&bytes) != hex_text {
                    return Err("WAL database record is not canonical hex".into());
                }
                if !record.is_null() {
                    return Err("WAL database frame also carries a JSON record".into());
                }
                let profile = crate::ContractProfile::from_setup(&transaction["state"])?;
                let projection = database_projection_with_profile(&bytes, profile)?;
                if db_artifacts_hash.is_some() && projection["status"] != "committed" {
                    return Err("WAL artifacts receipt on non-committed record".into());
                }
                journal.push(projection);
                raw_records.push(Some(bytes));
                db_artifacts_hashes.push(db_artifacts_hash);
            } else if !record.is_null() {
                validate_record(
                    record,
                    crate::ContractProfile::from_setup(&transaction["state"])?,
                )?;
                journal.push(record.clone());
                raw_records.push(None);
                db_artifacts_hashes.push(None);
            }
            state = Some(transaction.get("state").ok_or("missing WAL state")?.clone());
            previous = hash;
            complete = file.stream_position().map_err(|e| e.to_string())?;
        }
        file.set_len(complete)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        Ok(Self {
            file,
            state,
            journal,
            raw: raw_records,
            db_artifacts_hashes,
            previous,
            poisoned: false,
            phase: None,
            #[cfg(test)]
            before_commit: None,
        })
    }

    /// Dispatch is allowed only after this returns Ok. Any ambiguous I/O failure
    /// poisons this handle; reopen and recover identity rather than re-executing.
    pub fn commit(&mut self, state: &Value, record: Option<&Value>) -> Result<()> {
        #[cfg(test)]
        if let Some(hook) = &mut self.before_commit {
            hook();
        }
        if let Some(record) = record {
            validate_record(record, crate::ContractProfile::from_setup(state)?)?;
        }
        mark(&self.phase, Phase::EncodeEnter);
        let mut transaction = json!({"state":state, "journal":record, "previous":self.previous});
        let hash = digest(&cbor(&transaction)?);
        transaction["hash"] = Value::String(hash.clone());
        let raw = cbor(&transaction)?;
        mark(&self.phase, Phase::EncodeReturn);
        self.append(&raw, hash, state)?;
        if let Some(record) = record {
            self.journal.push(record.clone());
            self.raw.push(None);
            self.db_artifacts_hashes.push(None);
        }
        Ok(())
    }

    /// Atomically persist host state with one exact ABI 0.2 `db-step-record`.
    /// The bytes must be canonical, schema-valid and carry a correct hash; they
    /// are stored verbatim (see the type documentation).
    pub fn commit_database(&mut self, state: &Value, record: &[u8]) -> Result<()> {
        self.commit_database_receipt(state, record, None)
    }

    /// Publish a committed decision with an atomic receipt of its exact artifacts.
    #[cfg(feature = "native-engine")]
    pub(crate) fn commit_database_artifacts(
        &mut self,
        state: &Value,
        record: &[u8],
        artifacts: &[u8],
    ) -> Result<()> {
        self.commit_database_receipt(state, record, Some(digest(artifacts)))
    }

    fn commit_database_receipt(
        &mut self,
        state: &Value,
        record: &[u8],
        artifacts_hash: Option<String>,
    ) -> Result<()> {
        let projection =
            database_projection_with_profile(record, crate::ContractProfile::from_setup(state)?)?;
        if artifacts_hash.is_some() && projection["status"] != "committed" {
            return Err("artifacts receipt requires committed record".into());
        }
        let mut map = vec![
            (V::Text("state".into()), profile_value(state, 0)?),
            (V::Text("journal".into()), V::Null),
            (
                V::Text("previous".into()),
                profile_value(&self.previous, 0)?,
            ),
            (V::Text("db_record".into()), V::Text(hex::encode(record))),
        ];
        if let Some(hash) = &artifacts_hash {
            map.push((V::Text("db_artifacts_hash".into()), V::Text(hash.clone())));
        }
        let hash = digest(&coglet_types::encode_profile(&V::Map(map.clone()))?);
        map.push((V::Text("hash".into()), V::Text(hash.clone())));
        let raw = coglet_types::encode_profile(&V::Map(map))?;
        self.append(&raw, hash, state)?;
        self.journal.push(projection);
        self.raw.push(Some(record.to_vec()));
        self.db_artifacts_hashes.push(artifacts_hash);
        Ok(())
    }

    fn append(&mut self, raw: &[u8], hash: String, state: &Value) -> Result<()> {
        if self.poisoned {
            return Err("WAL requires recovery".into());
        }
        if raw.len() > MAX_TRANSACTION {
            return Err("resource_exhausted: transaction too large".into());
        }
        self.poisoned = true;
        mark(&self.phase, Phase::WalWriteEnter);
        self.file
            .write_all(&(raw.len() as u32).to_be_bytes())
            .and_then(|_| self.file.write_all(raw))
            .and_then(|_| {
                mark(&self.phase, Phase::WalWriteDone);
                mark(&self.phase, Phase::WalSyncEnter);
                self.file.sync_all()
            })
            .map_err(|e| e.to_string())?;
        mark(&self.phase, Phase::WalSyncDone);
        mark(&self.phase, Phase::CacheEnter);
        self.state = Some(state.clone());
        mark(&self.phase, Phase::CacheReturn);
        self.previous = Value::String(hash);
        self.poisoned = false;
        Ok(())
    }

    pub fn state(&self) -> Option<&Value> {
        self.state.as_ref()
    }
    pub(crate) fn is_healthy(&self) -> bool {
        !self.poisoned
    }
    pub fn records(&self) -> &[Value] {
        &self.journal
    }
    /// Exact ABI 0.2 bytes for entry `index` of `records()`, if it is one.
    pub fn database_record_bytes(&self, index: usize) -> Option<&[u8]> {
        self.raw.get(index).and_then(|r| r.as_deref())
    }

    /// Exact committed-publication artifact digest, absent on legacy/abort entries.
    pub fn database_artifacts_hash(&self, index: usize) -> Option<&str> {
        self.db_artifacts_hashes
            .get(index)
            .and_then(|hash| hash.as_deref())
    }

    /// Standard u32be + deterministic CBOR ABI journal framing.
    pub fn export_journal(&self, mut output: impl Write) -> Result<()> {
        for (record, exact) in self.journal.iter().zip(&self.raw) {
            let raw = match exact {
                Some(exact) => exact.clone(),
                None => cbor(record)?,
            };
            let n = u32::try_from(raw.len()).map_err(|_| "journal frame too large")?;
            output
                .write_all(&n.to_be_bytes())
                .and_then(|_| output.write_all(&raw))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn validate_record(record: &Value, profile: crate::ContractProfile) -> Result<()> {
    profile.validate(
        if record.get("step").is_some() {
            "step-record"
        } else if record["version"] == "0.2" {
            "database::db-boundary-record"
        } else {
            "boundary-record"
        },
        record,
    )?;
    let mut unhashed = record.clone();
    let claimed = unhashed
        .as_object_mut()
        .ok_or("invalid record")?
        .remove("record_hash")
        .ok_or("missing record hash")?;
    if claimed != Value::String(digest(&cbor(&unhashed)?)) {
        return Err("record hash mismatch".into());
    }
    Ok(())
}

/// Strictly validate exact ABI 0.2 record bytes (profile, closed schema,
/// canonical encoding, correct hash) and derive their JSON projection.
pub(crate) fn database_projection_with_profile(
    raw: &[u8],
    profile: crate::ContractProfile,
) -> Result<Value> {
    let record = crate::profile_codec::decode(profile.rule("database::db-step-record"), raw)?;
    let V::Map(entries) = &record else {
        return Err("database record is not a map".into());
    };
    let claimed = entries
        .iter()
        .find_map(|(k, v)| matches!(k, V::Text(t) if t == "record_hash").then_some(v))
        .ok_or("missing record hash")?;
    let unhashed: Vec<_> = entries
        .iter()
        .filter(|(k, _)| !matches!(k, V::Text(t) if t == "record_hash"))
        .cloned()
        .collect();
    if claimed != &V::Text(digest(&coglet_types::encode_profile(&V::Map(unhashed))?)) {
        return Err("record hash mismatch".into());
    }
    project(&record, 0)
}

/// Derived JSON view of a profile value (no byte strings exist in the profile).
pub(crate) fn project(value: &V, depth: usize) -> Result<Value> {
    if depth > coglet_types::MAX_DEPTH {
        return Err("profile nesting too deep".into());
    }
    Ok(match value {
        V::Null => Value::Null,
        V::Bool(b) => json!(b),
        V::Text(t) => json!(t),

        V::Integer(i) => {
            let i = i128::from(*i);
            if let Ok(n) = u64::try_from(i) {
                json!(n)
            } else {
                json!(i64::try_from(i).map_err(|_| "integer outside profile range")?)
            }
        }
        V::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| project(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        V::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(k, v)| match k {
                    V::Text(k) => Ok((k.clone(), project(v, depth + 1)?)),
                    _ => Err("non-text map key".to_string()),
                })
                .collect::<Result<_>>()?,
        ),
        _ => return Err("non-profile value".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A duplicated descriptor shares the open file description, exactly as
    /// the copy inherited by a child process does between `fork` and `exec`.
    /// Releasing the lock by `close` alone would keep the WAL owned for as
    /// long as that copy lives.
    #[test]
    fn dropping_the_store_releases_the_lock_while_a_shared_descriptor_lives() {
        let dir = tempfile::tempdir().unwrap();
        let wal = dir.path().join("state.wal");
        let owner = DurableStore::open(&wal).unwrap();
        let inherited = owner.file.try_clone().unwrap();
        let refused = DurableStore::open(&wal).err().expect("one owner");
        assert!(refused.starts_with("WAL already owned:"), "{refused}");
        drop(owner);
        let reopened = DurableStore::open(&wal);
        assert!(reopened.is_ok(), "{:?}", reopened.err());
        drop(inherited);
    }
}
