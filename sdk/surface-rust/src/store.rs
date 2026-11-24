//! Durable surface state: exclusive-ownership write-ahead ledger.
//!
//! One directory (mode 0700) holds `lock` (flock, exclusive), `identity` (the bound
//! surface id, the commit marker of initialisation), `ledger.log` (CRC-framed JSON
//! records, `fdatasync`ed before any acknowledgement or effect) and the optional
//! `kill.latch` file (written by the native kill path, see [`latch`]).
//!
//! Guarantees (and limits):
//! - Every `record_*`/`next_seq`/`set_epoch` returns only after the record is on stable
//!   storage as far as `sync_data` can promise. A failed append truncates the partial
//!   record and poisons the store; later writes fail closed.
//! - Torn final records (crash mid-append) are dropped on open. Corruption followed by
//!   more data, an identity mismatch, or a missing ledger next to an existing identity
//!   (lost ledger) fails closed: the caller must provision a NEW surface identity.
//! - On open, actions still `Pending` become terminal `failed/unavailable`; the
//!   uncertain effect is never replayed (deduplication, not exactly-once effects).
//! - Dedup entries are retained until 24 h (default) after the later of terminal time and
//!   deadline, measured on the wall clock because the surface monotonic clock restarts.
//!   Expired receipts are pruned/compacted on open and by the running service loop.
//! - `clean.shutdown` exists only before dispatch or after successful disconnected
//!   reconciliation. It is removed and the directory synced BEFORE connection start.
//!   A missing marker on restart is stopped even if kill.latch could not be written.
//!   This cannot promise a new durable write while the disk is broken: it relies on
//!   the running marker transition having succeeded before any dispatch.
//! - Event seq is reserved in blocks, so it is strictly increasing across restarts but
//!   may skip values (spec requires strictly increasing, not gap-free).

use crate::ids::{hex, unhex};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const RETENTION_US: u64 = 24 * 3_600 * 1_000_000;
const SEQ_BLOCK: u64 = 256;
const MAX_RECORD: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum StoreError {
    /// Another process (or Store) owns the directory.
    Busy,
    /// Identity file exists but the ledger does not, or the ledger is damaged.
    LostLedger(String),
    /// The directory belongs to a different surface identity.
    IdentityMismatch {
        stored: String,
        configured: String,
    },
    Corrupt(String),
    Io(String),
    /// A previous write failed; the store refuses further writes.
    Poisoned,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(
                f,
                "unavailable: state directory is owned by another instance"
            ),
            Self::LostLedger(m) => write!(
                f,
                "unavailable: ledger lost ({m}); reconcile and provision a new surface identity"
            ),
            Self::IdentityMismatch { stored, configured } => write!(
                f,
                "unauthenticated: state directory belongs to {stored}, configured {configured}"
            ),
            Self::Corrupt(m) => write!(f, "internal: ledger corrupt: {m}"),
            Self::Io(m) => write!(f, "internal: store I/O: {m}"),
            Self::Poisoned => write!(f, "internal: store poisoned by an earlier write failure"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryState {
    Rejected { error: Json, retain_until: u64 },
    Pending,
    Terminal { outcome: Json, retain_until: u64 },
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Complete deterministic CBOR `act` message exactly as first received.
    pub envelope: Vec<u8>,
    pub corr: String,
    pub grant_epoch: u64,
    pub deadline: u64,
    pub state: EntryState,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum Record {
    Init {
        surface: String,
    },
    Seq {
        upto: u64,
    },
    Epoch {
        epoch: u64,
    },
    Rejected {
        coglet: String,
        id: String,
        corr: String,
        envelope: String,
        grant_epoch: u64,
        deadline: u64,
        error: Json,
        retain_until: u64,
    },
    Admitted {
        coglet: String,
        id: String,
        corr: String,
        envelope: String,
        grant_epoch: u64,
        deadline: u64,
    },
    Terminal {
        coglet: String,
        id: String,
        outcome: Json,
        retain_until: u64,
    },
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn frame(rec: &Record) -> Result<Vec<u8>, StoreError> {
    let payload = serde_json::to_vec(rec).map_err(|e| StoreError::Io(e.to_string()))?;
    if payload.len() > MAX_RECORD {
        return Err(StoreError::Io("record too large".into()));
    }
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend((payload.len() as u32).to_be_bytes());
    out.extend(crc32(&payload).to_be_bytes());
    out.extend(payload);
    Ok(out)
}

fn sync_dir(dir: &Path) -> Result<(), StoreError> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub type WallClock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_wall_clock() -> WallClock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
    })
}

/// Path of the native-kill latch file inside a state directory.
pub fn latch_path(dir: &Path) -> PathBuf {
    dir.join("kill.latch")
}

/// Persist the kill latch. This is deliberately independent of [`Store`] (no lock, no
/// ledger): the native kill path must never wait on the control queue or the ledger.
pub fn latch(dir: &Path) -> std::io::Result<()> {
    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(latch_path(dir))?;
    f.sync_all()?;
    File::open(dir)?.sync_all()
}

pub struct Store {
    dir: PathBuf,
    _lock: File,
    log: File,
    surface: String,
    wall: WallClock,
    entries: BTreeMap<(String, String), Entry>,
    seq_next: u64,
    seq_reserved: u64,
    epoch: u64,
    recovered: Vec<(String, String)>,
    poisoned: bool,
    startup_unclean: bool,
}

impl Store {
    pub fn open(dir: &Path, surface: &str) -> Result<Self, StoreError> {
        Self::open_with_clock(dir, surface, system_wall_clock())
    }

    pub fn open_with_clock(dir: &Path, surface: &str, wall: WallClock) -> Result<Self, StoreError> {
        if !dir.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        let md = fs::metadata(dir)?;
        if !md.is_dir() {
            return Err(StoreError::Io("state path is not a directory".into()));
        }
        if md.permissions().mode() & 0o077 != 0 {
            // Tighten when we own it; refuse if we cannot make it private.
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| StoreError::Io(format!("state directory not private (0700): {e}")))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(dir.join("lock"))?;
        // SAFETY: valid fd owned by `lock`.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(StoreError::Busy);
        }
        let identity_path = dir.join("identity");
        let log_path = dir.join("ledger.log");
        let identity = match fs::read_to_string(&identity_path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let log_exists = log_path.exists();
        match (&identity, log_exists) {
            (Some(id), _) if id != surface => {
                return Err(StoreError::IdentityMismatch {
                    stored: id.clone(),
                    configured: surface.to_owned(),
                });
            }
            (Some(_), false) => return Err(StoreError::LostLedger("ledger.log missing".into())),
            (None, true) if fs::metadata(&log_path)?.len() > 0 => {
                return Err(StoreError::LostLedger("identity missing".into()));
            }
            _ => {}
        }
        let mut store = Store {
            dir: dir.to_owned(),
            _lock: lock,
            log: OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .mode(0o600)
                .open(&log_path)?,
            surface: surface.to_owned(),
            wall,
            entries: BTreeMap::new(),
            seq_next: 0,
            seq_reserved: 0,
            epoch: 0,
            recovered: Vec::new(),
            poisoned: false,
            startup_unclean: identity.is_some() && !dir.join("clean.shutdown").is_file(),
        };
        if identity.is_none() {
            // Fresh state: Init record is made durable before the identity marker.
            store.append(&Record::Init {
                surface: surface.to_owned(),
            })?;
            sync_dir(dir)?;
            let tmp = dir.join("identity.tmp");
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(surface.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &identity_path)?;
            sync_dir(dir)?;
        } else {
            store.replay()?;
        }
        if identity.is_none() {
            store.mark_clean()?;
        }
        store.recover_pending()?;
        store.prune_and_compact()?;
        Ok(store)
    }

    fn replay(&mut self) -> Result<(), StoreError> {
        let mut data = Vec::new();
        self.log.seek_start()?;
        self.log.read_to_end(&mut data)?;
        let mut pos = 0usize;
        let mut saw_init = false;
        while pos < data.len() {
            let rest = &data[pos..];
            // A bad record is a torn tail only when nothing valid can follow it.
            if rest.len() < 8 {
                break;
            }
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let crc = u32::from_be_bytes(rest[4..8].try_into().unwrap());
            if len > MAX_RECORD {
                return Err(StoreError::Corrupt(format!("record length {len} at {pos}")));
            }
            if rest.len() < 8 + len {
                break;
            }
            let payload = &rest[8..8 + len];
            if crc32(payload) != crc {
                if rest.len() == 8 + len {
                    break;
                }
                return Err(StoreError::Corrupt(format!("checksum mismatch at {pos}")));
            }
            let rec: Record = serde_json::from_slice(payload)
                .map_err(|e| StoreError::Corrupt(format!("record at {pos}: {e}")))?;
            self.apply(rec, &mut saw_init)?;
            pos += 8 + len;
        }
        if !saw_init {
            return Err(StoreError::LostLedger("ledger has no init record".into()));
        }
        if pos < data.len() {
            self.log.set_len(pos as u64)?;
            self.log.sync_all()?;
        }
        self.seq_next = self.seq_reserved;
        Ok(())
    }

    fn apply(&mut self, rec: Record, saw_init: &mut bool) -> Result<(), StoreError> {
        match rec {
            Record::Init { surface } => {
                if surface != self.surface {
                    return Err(StoreError::IdentityMismatch {
                        stored: surface,
                        configured: self.surface.clone(),
                    });
                }
                *saw_init = true;
            }
            Record::Seq { upto } => self.seq_reserved = self.seq_reserved.max(upto),
            Record::Epoch { epoch } => self.epoch = self.epoch.max(epoch),
            Record::Admitted {
                coglet,
                id,
                corr,
                envelope,
                grant_epoch,
                deadline,
            } => {
                let envelope = unhex(&envelope).ok_or_else(|| StoreError::Corrupt("hex".into()))?;
                self.entries.insert(
                    (coglet, id),
                    Entry {
                        envelope,
                        corr,
                        grant_epoch,
                        deadline,
                        state: EntryState::Pending,
                    },
                );
            }
            Record::Rejected {
                coglet,
                id,
                corr,
                envelope,
                grant_epoch,
                deadline,
                error,
                retain_until,
            } => {
                let envelope = unhex(&envelope).ok_or_else(|| StoreError::Corrupt("hex".into()))?;
                self.entries.insert(
                    (coglet, id),
                    Entry {
                        envelope,
                        corr,
                        grant_epoch,
                        deadline,
                        state: EntryState::Rejected {
                            error,
                            retain_until,
                        },
                    },
                );
            }
            Record::Terminal {
                coglet,
                id,
                outcome,
                retain_until,
            } => {
                let e = self
                    .entries
                    .get_mut(&(coglet, id))
                    .ok_or_else(|| StoreError::Corrupt("terminal without admission".into()))?;
                e.state = EntryState::Terminal {
                    outcome,
                    retain_until,
                };
            }
        }
        Ok(())
    }

    fn append(&mut self, rec: &Record) -> Result<(), StoreError> {
        self.append_batch(std::slice::from_ref(rec))
    }

    // Existing independently framed records, one durability barrier. If a crash
    // leaves only a prefix, replay accepts that prefix; no caller has received
    // acknowledgement until the entire batch has reached stable storage.
    fn append_batch(&mut self, records: &[Record]) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Poisoned);
        }
        let mut bytes = Vec::new();
        for rec in records {
            bytes.extend(frame(rec)?);
        }
        let before = self.log.metadata()?.len();
        let r = self
            .log
            .write_all(&bytes)
            .and_then(|()| self.log.sync_data());
        if let Err(e) = r {
            self.poisoned = true;
            let _ = self.log.set_len(before);
            let _ = self.log.sync_all();
            return Err(e.into());
        }
        Ok(())
    }

    fn recover_pending(&mut self) -> Result<(), StoreError> {
        let pending: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| e.state == EntryState::Pending)
            .map(|(k, e)| (k.clone(), e.corr.clone()))
            .collect();
        for ((coglet, id), corr) in pending {
            let outcome = json!({
                "id": id, "correlation_id": corr, "status": "failed",
                "error": {"code": "unavailable", "retryable": false,
                          "message": "surface restarted before completion; physical effect state unknown, not replayed"}
            });
            // The old monotonic deadline cannot be translated after restart.
            // Conservatively retain the entire maximum admission horizon as well.
            let retain = self
                .wall_now()
                .saturating_add(RETENTION_US)
                .saturating_add(60_000_000);
            self.record_terminal(&coglet, &id, outcome, retain)?;
            self.recovered.push((coglet, id));
        }
        Ok(())
    }

    pub(crate) fn prune_and_compact(&mut self) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Poisoned);
        }
        let result = self.compact_expired();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn compact_expired(&mut self) -> Result<(), StoreError> {
        let now = (self.wall)();
        let before = self.entries.len();
        self.entries.retain(|_, e| match &e.state {
            EntryState::Rejected { retain_until, .. }
            | EntryState::Terminal { retain_until, .. } => *retain_until > now,
            EntryState::Pending => true,
        });
        if self.entries.len() == before {
            return Ok(());
        }
        let mut out = frame(&Record::Init {
            surface: self.surface.clone(),
        })?;
        out.extend(frame(&Record::Seq {
            upto: self.seq_reserved,
        })?);
        out.extend(frame(&Record::Epoch { epoch: self.epoch })?);
        for ((coglet, id), e) in &self.entries {
            let envelope = hex(&e.envelope);
            let (coglet, id, corr) = (coglet.clone(), id.clone(), e.corr.clone());
            match &e.state {
                EntryState::Rejected {
                    error,
                    retain_until,
                } => out.extend(frame(&Record::Rejected {
                    coglet,
                    id,
                    corr,
                    envelope,
                    grant_epoch: e.grant_epoch,
                    deadline: e.deadline,
                    error: error.clone(),
                    retain_until: *retain_until,
                })?),
                EntryState::Pending => out.extend(frame(&Record::Admitted {
                    coglet,
                    id,
                    corr,
                    envelope,
                    grant_epoch: e.grant_epoch,
                    deadline: e.deadline,
                })?),
                EntryState::Terminal {
                    outcome,
                    retain_until,
                } => {
                    out.extend(frame(&Record::Admitted {
                        coglet: coglet.clone(),
                        id: id.clone(),
                        corr,
                        envelope,
                        grant_epoch: e.grant_epoch,
                        deadline: e.deadline,
                    })?);
                    out.extend(frame(&Record::Terminal {
                        coglet,
                        id,
                        outcome: outcome.clone(),
                        retain_until: *retain_until,
                    })?);
                }
            }
        }
        let tmp = self.dir.join("ledger.log.tmp");
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
        fs::rename(&tmp, self.dir.join("ledger.log"))?;
        sync_dir(&self.dir)?;
        self.log = OpenOptions::new()
            .read(true)
            .append(true)
            .open(self.dir.join("ledger.log"))?;
        Ok(())
    }

    pub fn wall_now(&self) -> u64 {
        (self.wall)()
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn lookup(&self, coglet: &str, id: &str) -> Option<&Entry> {
        self.entries.get(&(coglet.to_owned(), id.to_owned()))
    }
    pub fn entries(&self) -> impl Iterator<Item = (&(String, String), &Entry)> {
        self.entries.iter()
    }
    /// Actions that were pending at open and have been failed/unavailable.
    pub fn recovered(&self) -> &[(String, String)] {
        &self.recovered
    }
    pub fn grant_epoch(&self) -> u64 {
        self.epoch
    }
    pub fn latched(&self) -> bool {
        self.startup_unclean || latch_path(&self.dir).exists()
    }
    /// Explicit local operator reset. Callers must also require a fresh negotiation.
    pub fn clear_latch(&mut self) -> Result<(), StoreError> {
        match fs::remove_file(latch_path(&self.dir)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        sync_dir(&self.dir)?;
        self.mark_clean()?;
        self.startup_unclean = false;
        Ok(())
    }

    /// Remove the clean marker DURABLY before any connected dispatch. If a later
    /// kill write fails, absence of this marker still fences the next process.
    pub(crate) fn mark_running(&mut self) -> Result<(), StoreError> {
        match fs::remove_file(self.dir.join("clean.shutdown")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        sync_dir(&self.dir)
    }

    pub(crate) fn mark_clean(&mut self) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Poisoned);
        }
        let f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(self.dir.join("clean.shutdown"))?;
        f.sync_all()?;
        sync_dir(&self.dir)
    }

    pub fn set_epoch(&mut self, epoch: u64) -> Result<(), StoreError> {
        if epoch <= self.epoch {
            return Err(StoreError::Corrupt(
                "grant epoch must strictly increase".into(),
            ));
        }
        self.append(&Record::Epoch { epoch })?;
        self.epoch = epoch;
        Ok(())
    }

    /// Next event sequence number (starts at 0; strictly increases across restarts).
    pub fn next_seq(&mut self) -> Result<u64, StoreError> {
        if self.seq_next >= self.seq_reserved {
            let upto = self
                .seq_next
                .checked_add(SEQ_BLOCK)
                .ok_or(StoreError::Corrupt("seq overflow".into()))?;
            self.append(&Record::Seq { upto })?;
            self.seq_reserved = upto;
        }
        let s = self.seq_next;
        self.seq_next += 1;
        Ok(s)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_rejection(
        &mut self,
        coglet: &str,
        id: &str,
        corr: &str,
        envelope: &[u8],
        grant_epoch: u64,
        deadline: u64,
        error: Json,
        retain_until: u64,
    ) -> Result<(), StoreError> {
        self.append(&Record::Rejected {
            coglet: coglet.into(),
            id: id.into(),
            corr: corr.into(),
            envelope: hex(envelope),
            grant_epoch,
            deadline,
            error: error.clone(),
            retain_until,
        })?;
        self.entries.insert(
            (coglet.into(), id.into()),
            Entry {
                envelope: envelope.to_vec(),
                corr: corr.into(),
                grant_epoch,
                deadline,
                state: EntryState::Rejected {
                    error,
                    retain_until,
                },
            },
        );
        Ok(())
    }

    pub fn record_admission(
        &mut self,
        coglet: &str,
        id: &str,
        corr: &str,
        envelope: &[u8],
        grant_epoch: u64,
        deadline: u64,
    ) -> Result<(), StoreError> {
        self.append(&Record::Admitted {
            coglet: coglet.into(),
            id: id.into(),
            corr: corr.into(),
            envelope: hex(envelope),
            grant_epoch,
            deadline,
        })?;
        self.entries.insert(
            (coglet.into(), id.into()),
            Entry {
                envelope: envelope.to_vec(),
                corr: corr.into(),
                grant_epoch,
                deadline,
                state: EntryState::Pending,
            },
        );
        Ok(())
    }

    /// Terminal outcome for an admitted, still-pending action. `retain_until` 0 means
    /// "default: wall now + retention".
    pub fn record_terminal(
        &mut self,
        coglet: &str,
        id: &str,
        outcome: Json,
        retain_until: u64,
    ) -> Result<(), StoreError> {
        let key = (coglet.to_owned(), id.to_owned());
        match self.entries.get(&key).map(|e| &e.state) {
            Some(EntryState::Pending) => {}
            _ => {
                return Err(StoreError::Corrupt(
                    "terminal for non-pending action".into(),
                ));
            }
        }
        let retain_until = if retain_until == 0 {
            (self.wall)().saturating_add(RETENTION_US)
        } else {
            retain_until
        };
        let mut records = vec![Record::Terminal {
            coglet: coglet.into(),
            id: id.into(),
            outcome: outcome.clone(),
            retain_until,
        }];
        // A terminal will normally be delivered as an event next. Reserve its
        // sequence in the SAME sync, not an extra sync on the response path.
        // Do not consume it: delivery (and replay) still assigns fresh seqs.
        let reserve = if self.seq_next >= self.seq_reserved {
            let upto = self
                .seq_next
                .checked_add(SEQ_BLOCK)
                .ok_or_else(|| StoreError::Corrupt("seq overflow".into()))?;
            records.push(Record::Seq { upto });
            Some(upto)
        } else {
            None
        };
        self.append_batch(&records)?;
        if let Some(upto) = reserve {
            self.seq_reserved = upto;
        }
        if let Some(e) = self.entries.get_mut(&key) {
            e.state = EntryState::Terminal {
                outcome,
                retain_until,
            };
        }
        Ok(())
    }
}

trait SeekStart {
    fn seek_start(&mut self) -> std::io::Result<()>;
}
impl SeekStart for File {
    fn seek_start(&mut self) -> std::io::Result<()> {
        use std::io::Seek;
        self.seek(std::io::SeekFrom::Start(0)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "coglet-surface-store-{}-{}-{}",
            std::process::id(),
            tag,
            crate::ids::random_id().unwrap()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }
    const S: &str = "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn live_pruning_preserves_pending_and_reclaims_expired_receipts() {
        let d = tmpdir("live-prune");
        let mut s = Store::open_with_clock(&d, S, Arc::new(|| 1_000)).unwrap();
        s.record_rejection("c", "old", "corr", b"env", 1, 1, json!({}), 999)
            .unwrap();
        s.record_admission("c", "running", "corr", b"env", 1, 10)
            .unwrap();
        s.prune_and_compact().unwrap();
        assert!(s.lookup("c", "old").is_none());
        assert_eq!(s.lookup("c", "running").unwrap().state, EntryState::Pending);
        drop(s);
        let s = Store::open_with_clock(&d, S, Arc::new(|| 1_000)).unwrap();
        assert!(s.lookup("c", "running").is_some());
    }

    #[test]
    fn recovery_retains_unknown_clock_deadline_for_full_maximum_window() {
        let d = tmpdir("recovery-retain");
        let mut s = Store::open_with_clock(&d, S, Arc::new(|| 1_000)).unwrap();
        s.record_admission("c", "id", "corr", b"env", 1, 60_000_000)
            .unwrap();
        drop(s);
        let s = Store::open_with_clock(&d, S, Arc::new(|| 2_000)).unwrap();
        match s.lookup("c", "id").unwrap().state {
            EntryState::Terminal { retain_until, .. } => {
                assert!(retain_until >= 2_000 + RETENTION_US + 60_000_000)
            }
            _ => panic!("not recovered"),
        }
    }

    #[test]
    fn exclusive_ownership_and_identity() {
        let d = tmpdir("excl");
        let s = Store::open(&d, S).unwrap();
        assert!(matches!(Store::open(&d, S), Err(StoreError::Busy)));
        drop(s);
        let other = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        assert!(matches!(
            Store::open(&d, other),
            Err(StoreError::IdentityMismatch { .. })
        ));
        Store::open(&d, S).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn lost_ledger_fails_closed() {
        let d = tmpdir("lost");
        drop(Store::open(&d, S).unwrap());
        fs::remove_file(d.join("ledger.log")).unwrap();
        assert!(matches!(Store::open(&d, S), Err(StoreError::LostLedger(_))));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn terminal_reserves_delivery_sequence_before_returning() {
        let d = tmpdir("terminal-seq");
        let mut s = Store::open(&d, S).unwrap();
        s.record_admission("c", "id", "corr", b"env", 1, 99)
            .unwrap();
        s.record_terminal("c", "id", json!({"status": "succeeded"}), 0)
            .unwrap();
        assert!(
            s.seq_reserved > s.seq_next,
            "terminal commit must include delivery reservation"
        );
        let log_size = s.log.metadata().unwrap().len();
        assert_eq!(s.next_seq().unwrap(), 0);
        assert_eq!(
            s.log.metadata().unwrap().len(),
            log_size,
            "delivery must not append another record"
        );
        drop(s);
        let mut s = Store::open(&d, S).unwrap();
        assert!(
            s.next_seq().unwrap() >= SEQ_BLOCK,
            "restart must skip the durable reservation"
        );
        assert!(matches!(
            s.lookup("c", "id").unwrap().state,
            EntryState::Terminal { .. }
        ));
        drop(s);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn terminal_batch_failure_does_not_publish_memory_state() {
        let d = tmpdir("terminal-failure");
        let mut s = Store::open(&d, S).unwrap();
        s.record_admission("c", "id", "corr", b"env", 1, 99)
            .unwrap();
        // Force a real write error, not a mocked successful durability barrier.
        s.log = OpenOptions::new()
            .read(true)
            .open(d.join("ledger.log"))
            .unwrap();
        assert!(s.record_terminal("c", "id", json!({}), 0).is_err());
        assert!(s.poisoned);
        assert_eq!(s.seq_reserved, 0);
        assert!(matches!(
            s.lookup("c", "id").unwrap().state,
            EntryState::Pending
        ));
        assert!(s.next_seq().is_err());
        drop(s);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn terminal_batch_replay_accepts_a_torn_sequence_tail() {
        let d = tmpdir("terminal-torn-seq");
        let mut s = Store::open(&d, S).unwrap();
        s.record_admission("c", "id", "corr", b"env", 1, 99)
            .unwrap();
        s.record_terminal("c", "id", json!({"status": "succeeded"}), 0)
            .unwrap();
        // Simulate a crash after Terminal reached disk but Seq did not finish.
        let length = s.log.metadata().unwrap().len();
        s.log.set_len(length - 1).unwrap();
        drop(s);
        let mut s = Store::open(&d, S).unwrap();
        assert!(matches!(
            s.lookup("c", "id").unwrap().state,
            EntryState::Terminal { .. }
        ));
        // Nothing was acknowledged before the batch barrier, so reusing zero
        // here is safe; the terminal result is still available for reconciliation.
        assert_eq!(s.next_seq().unwrap(), 0);
        for expected in 1..SEQ_BLOCK {
            assert_eq!(s.next_seq().unwrap(), expected);
        }
        s.record_admission("c", "id2", "corr", b"env2", 1, 99)
            .unwrap();
        s.record_terminal("c", "id2", json!({}), 0).unwrap();
        let length = s.log.metadata().unwrap().len();
        assert_eq!(s.next_seq().unwrap(), SEQ_BLOCK);
        assert_eq!(s.log.metadata().unwrap().len(), length);
        drop(s);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn seq_epoch_and_pending_recovery_persist() {
        let d = tmpdir("persist");
        let mut s = Store::open(&d, S).unwrap();
        assert_eq!(s.next_seq().unwrap(), 0);
        assert_eq!(s.next_seq().unwrap(), 1);
        s.set_epoch(3).unwrap();
        assert!(s.set_epoch(3).is_err());
        s.record_admission("coglet:x", "id1", "c1", b"env", 3, 99)
            .unwrap();
        drop(s);
        let mut s = Store::open(&d, S).unwrap();
        assert!(s.next_seq().unwrap() >= 2, "seq never regresses");
        assert_eq!(s.grant_epoch(), 3);
        assert_eq!(s.recovered(), &[("coglet:x".to_owned(), "id1".to_owned())]);
        let e = s.lookup("coglet:x", "id1").unwrap();
        assert_eq!(e.envelope, b"env");
        match &e.state {
            EntryState::Terminal { outcome, .. } => {
                assert_eq!(outcome["status"], "failed");
                assert_eq!(outcome["error"]["code"], "unavailable");
            }
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn torn_tail_dropped_but_midfile_corruption_fails_closed() {
        let d = tmpdir("torn");
        let mut s = Store::open(&d, S).unwrap();
        s.record_admission("coglet:x", "id1", "c1", b"env", 1, 9)
            .unwrap();
        drop(s);
        let log = d.join("ledger.log");
        let good = fs::read(&log).unwrap();
        // torn: a partial extra record
        let mut torn = good.clone();
        torn.extend([0, 0, 0, 40, 1, 2, 3]);
        fs::write(&log, &torn).unwrap();
        let s = Store::open(&d, S).unwrap();
        assert!(s.lookup("coglet:x", "id1").is_some());
        drop(s);
        // corrupt a byte in the first record (followed by valid data)
        let mut bad = fs::read(&log).unwrap();
        bad[10] ^= 0xff;
        fs::write(&log, &bad).unwrap();
        assert!(matches!(Store::open(&d, S), Err(StoreError::Corrupt(_))));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn retention_prunes_and_compacts_after_24h() {
        let d = tmpdir("retain");
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let c2 = clock.clone();
        let wall: WallClock = Arc::new(move || c2.load(std::sync::atomic::Ordering::SeqCst));
        let mut s = Store::open_with_clock(&d, S, wall.clone()).unwrap();
        s.record_admission("coglet:x", "id1", "c1", b"env", 1, 9)
            .unwrap();
        s.record_terminal("coglet:x", "id1", json!({"status":"succeeded"}), 0)
            .unwrap();
        drop(s);
        clock.store(
            1_000 + RETENTION_US - 1,
            std::sync::atomic::Ordering::SeqCst,
        );
        assert!(
            Store::open_with_clock(&d, S, wall.clone())
                .unwrap()
                .lookup("coglet:x", "id1")
                .is_some()
        );
        clock.store(
            1_000 + RETENTION_US + 1,
            std::sync::atomic::Ordering::SeqCst,
        );
        let s = Store::open_with_clock(&d, S, wall.clone()).unwrap();
        assert!(s.lookup("coglet:x", "id1").is_none());
        drop(s);
        // compacted ledger still reopens and keeps identity/epoch state
        Store::open_with_clock(&d, S, wall).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn open_directory_is_tightened_to_private() {
        let d = tmpdir("perm");
        fs::create_dir_all(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        Store::open(&d, S).unwrap();
        assert_eq!(
            fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_dir_all(&d);
    }
}
