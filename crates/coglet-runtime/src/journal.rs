use crate::{Result, cbor, digest, validate};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const MAX_TRANSACTION: usize = 64 * 1024 * 1024;

/// Internal WAL, NOT the exported ABI journal. One fsynced frame atomically
/// holds all runtime host state and an optional ABI record. The ABI stream is
/// exported separately. An exclusive file lock prevents concurrent writers.
/// The supervisor must provide a private persistent parent directory.
pub struct DurableStore {
    file: File,
    state: Option<Value>,
    journal: Vec<Value>,
    previous: Value,
    poisoned: bool,
}

impl DurableStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path).map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|e| format!("WAL already owned: {e}"))?;
        // Also persist a newly created directory entry before admitting work.
        file.sync_all().map_err(|e| e.to_string())?;
        File::open(path.parent().ok_or("WAL needs a parent directory")?)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        let mut state = None;
        let mut journal = Vec::new();
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
            let mut transaction: Value = coglet_types::decode_profile(&raw)?
                .deserialized()
                .map_err(|e| e.to_string())?;
            let object = transaction
                .as_object_mut()
                .ok_or("WAL transaction is not a map")?;
            if object.len() != 4 {
                return Err("unexpected WAL fields".into());
            }
            let hash = object.remove("hash").ok_or("missing WAL checksum")?;
            if object.get("previous") != Some(&previous)
                || hash != Value::String(digest(&cbor(&transaction)?))
            {
                return Err("WAL checksum/chain mismatch".into());
            }
            let record = transaction.get("journal").ok_or("missing WAL journal")?;
            if !record.is_null() {
                validate_record(record)?;
                journal.push(record.clone());
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
            previous,
            poisoned: false,
        })
    }

    /// Dispatch is allowed only after this returns Ok. Any ambiguous I/O failure
    /// poisons this handle; reopen and recover identity rather than re-executing.
    pub fn commit(&mut self, state: &Value, record: Option<&Value>) -> Result<()> {
        if self.poisoned {
            return Err("WAL requires recovery".into());
        }
        if let Some(record) = record {
            validate_record(record)?;
        }
        let mut transaction = json!({"state":state, "journal":record, "previous":self.previous});
        let hash = digest(&cbor(&transaction)?);
        transaction["hash"] = Value::String(hash.clone());
        let raw = cbor(&transaction)?;
        if raw.len() > MAX_TRANSACTION {
            return Err("resource_exhausted: transaction too large".into());
        }
        self.poisoned = true;
        self.file
            .write_all(&(raw.len() as u32).to_be_bytes())
            .and_then(|_| self.file.write_all(&raw))
            .and_then(|_| self.file.sync_all())
            .map_err(|e| e.to_string())?;
        self.state = Some(state.clone());
        self.previous = Value::String(hash);
        if let Some(record) = record {
            self.journal.push(record.clone());
        }
        self.poisoned = false;
        Ok(())
    }

    pub fn state(&self) -> Option<&Value> {
        self.state.as_ref()
    }
    pub fn records(&self) -> &[Value] {
        &self.journal
    }

    /// Standard u32be + deterministic CBOR ABI journal framing.
    pub fn export_journal(&self, mut output: impl Write) -> Result<()> {
        for record in &self.journal {
            let raw = cbor(record)?;
            let n = u32::try_from(raw.len()).map_err(|_| "journal frame too large")?;
            output
                .write_all(&n.to_be_bytes())
                .and_then(|_| output.write_all(&raw))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn validate_record(record: &Value) -> Result<()> {
    validate(
        if record.get("step").is_some() {
            "step-record"
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
