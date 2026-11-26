//! Copy-on-write guest files and atomic, content-addressed checkpoints.
//!
//! This is a filesystem primitive, not the runtime journal/outbox coordinator.
//! `Snapshot` is immutable; drop a `Transaction` to abort. Host boundary methods
//! are separate from guest methods; authentication, typed inbox/heard/log schema
//! validation, audit recording and step serialization belong to the runtime.
mod disk;
mod git;
mod state_hash;
pub use disk::{DiskStore, FlushPoint};
pub use git::{GitPoint, GitStore};
use sha2::{Digest, Sha256};
pub use state_hash::snapshot_tree_hash;
use std::{collections::BTreeMap, sync::Arc};

pub const INBOX: &str = "/data/inbox.jsonl";
pub const HEARD: &str = "/data/runtime/heard.cbor";
pub const EVENTS: &str = "/logs/events.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// WASI permission errno: includes forbidden/malformed guest paths.
    PermissionDenied,
    NotFound,
    Conflict,
    InvalidArgument,
    ResourceExhausted,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2)
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}
pub fn validate_path(path: &str) -> Result<()> {
    if path.bytes().any(|b| b < 32 || b == 127 || b == b'\\') {
        return Err(Error::PermissionDenied);
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() < 3
        || !parts[0].is_empty()
        || !matches!(parts[1], "code" | "data" | "logs")
        || parts[2..]
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".." || p.eq_ignore_ascii_case(".git"))
    {
        return Err(Error::PermissionDenied);
    }
    Ok(())
}
fn guest_writable(path: &str, append: bool) -> Result<()> {
    validate_path(path)?;
    if path.starts_with("/code/")
        || matches!(path, INBOX | HEARD | EVENTS)
        || reserved_alias(path)
        || (path.starts_with("/logs/") && !append)
    {
        return Err(Error::PermissionDenied);
    }
    Ok(())
}

// Protect implicit reserved directories even before the first host write.
fn reserved_alias(path: &str) -> bool {
    [INBOX, HEARD, EVENTS].iter().any(|reserved| {
        path != *reserved
            && (path
                .strip_prefix(reserved)
                .is_some_and(|s| s.starts_with('/'))
                || reserved
                    .strip_prefix(path)
                    .is_some_and(|s| s.starts_with('/')))
    })
}

#[derive(Clone, Debug)]
struct Blob {
    bytes: Arc<[u8]>,
    hash: String,
}
impl Blob {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.into(),
            hash: digest(bytes),
        }
    }
}
/// Cloning shares both the tree and immutable file blobs. First mutation copies
/// only tree entries; untouched file contents remain shared across snapshots.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    files: Arc<BTreeMap<String, Blob>>,
}
impl Snapshot {
    /// Trusted bootstrap/restore, not a guest import or a code admission check.
    pub fn from_files(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let mut snapshot = Self::default();
        for (path, bytes) in files {
            snapshot.put(&path, &bytes)?;
        }
        Ok(snapshot)
    }
    pub fn read(&self, path: &str) -> Result<&[u8]> {
        validate_path(path)?;
        self.files
            .get(path)
            .map(|b| b.bytes.as_ref())
            .ok_or(Error::NotFound)
    }
    pub fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files
            .iter()
            .map(|(p, b)| (p.as_str(), b.bytes.as_ref()))
    }
    pub fn blob_hash(&self, path: &str) -> Result<&str> {
        validate_path(path)?;
        self.files
            .get(path)
            .map(|b| b.hash.as_str())
            .ok_or(Error::NotFound)
    }
    /// ABI snapshot: paths to lowercase hex, including every mount.
    pub fn export(&self) -> BTreeMap<String, String> {
        self.files().map(|(p, b)| (p.to_owned(), hex(b))).collect()
    }
    pub fn tree_hash(&self) -> String {
        snapshot_tree_hash(&self.export()).expect("validated filesystem snapshot")
    }
    pub fn cbor(&self) -> Vec<u8> {
        let mut entries: Vec<_> = self
            .export()
            .into_iter()
            .map(|(p, b)| (text(&p), text(&b)))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = Vec::new();
        major(&mut out, 5, entries.len() as u64);
        for (k, v) in entries {
            out.extend(k);
            out.extend(v);
        }
        out
    }
    pub fn transaction(&self) -> Transaction {
        Transaction { view: self.clone() }
    }
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        validate_path(path)?;
        if reserved_alias(path) {
            return Err(Error::PermissionDenied);
        }
        // Only regular files and implicit directories; no file/dir aliasing.
        let prefix = format!("{path}/");
        let descendant = self
            .files
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(p, _)| p.starts_with(&prefix));
        let ancestor = path
            .match_indices('/')
            .any(|(i, _)| self.files.contains_key(&path[..i]));
        if ancestor || descendant {
            return Err(Error::Conflict);
        }
        Arc::make_mut(&mut self.files).insert(path.to_owned(), Blob::new(bytes));
        Ok(())
    }
}
fn major(out: &mut Vec<u8>, kind: u8, n: u64) {
    let base = kind << 5;
    match n {
        0..=23 => out.push(base | n as u8),
        24..=255 => out.extend([base | 24, n as u8]),
        256..=65535 => {
            out.push(base | 25);
            out.extend((n as u16).to_be_bytes());
        }
        65536..=4294967295 => {
            out.push(base | 26);
            out.extend((n as u32).to_be_bytes());
        }
        _ => {
            out.push(base | 27);
            out.extend(n.to_be_bytes());
        }
    }
}
fn text(s: &str) -> Vec<u8> {
    let mut b = Vec::new();
    major(&mut b, 3, s.len() as u64);
    b.extend(s.as_bytes());
    b
}

#[cfg(test)]
mod unit_tests;

pub struct Transaction {
    view: Snapshot,
}
impl Transaction {
    pub fn snapshot(&self) -> &Snapshot {
        &self.view
    }
    pub fn commit(self) -> Snapshot {
        self.view
    }
    pub fn write(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        guest_writable(path, false)?;
        self.view.put(path, bytes)
    }
    pub fn create(&mut self, path: &str) -> Result<()> {
        guest_writable(path, true)?;
        if self.view.files.contains_key(path) {
            return Err(Error::Conflict);
        }
        self.view.put(path, &[])
    }
    pub fn append(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        guest_writable(path, true)?;
        self.append_unchecked(path, bytes)
    }
    pub fn write_at(&mut self, path: &str, offset: usize, bytes: &[u8]) -> Result<()> {
        guest_writable(path, false)?;
        let end = offset
            .checked_add(bytes.len())
            .ok_or(Error::InvalidArgument)?;
        let mut data = self.view.read(path)?.to_vec();
        // Regular-file writes may create zero-filled holes. Runtime adapters
        // must admit aggregate host-byte budgets before calling this primitive.
        data.try_reserve(end.saturating_sub(data.len()))
            .map_err(|_| Error::ResourceExhausted)?;
        data.resize(end.max(data.len()), 0);
        data[offset..end].copy_from_slice(bytes);
        self.view.put(path, &data)
    }
    pub fn truncate(&mut self, path: &str, len: usize) -> Result<()> {
        guest_writable(path, false)?;
        let mut bytes = self.view.read(path)?.to_vec();
        bytes
            .try_reserve(len.saturating_sub(bytes.len()))
            .map_err(|_| Error::ResourceExhausted)?;
        bytes.resize(len, 0);
        self.view.put(path, &bytes)
    }
    pub fn unlink(&mut self, path: &str) -> Result<()> {
        guest_writable(path, false)?;
        Arc::make_mut(&mut self.view.files)
            .remove(path)
            .ok_or(Error::NotFound)?;
        Ok(())
    }
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        guest_writable(from, false)?;
        guest_writable(to, false)?;
        let data = self.view.read(from)?.to_vec();
        if from == to {
            return Ok(());
        }
        // Check destination before removing source; failures have no mutation.
        self.view.put(to, &data)?;
        Arc::make_mut(&mut self.view.files).remove(from);
        Ok(())
    }
    /// Boundary-only trusted debug replacement; never permits reserved paths.
    pub fn debug_write(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        validate_path(path)?;
        if !path.starts_with("/data/") || matches!(path, INBOX | HEARD) {
            return Err(Error::PermissionDenied);
        }
        self.view.put(path, bytes)
    }
    /// Runtime already validated the typed message, canonical JSONL, index/id,
    /// pending-work transition and authorization. Preserve its exact bytes.
    pub fn host_append_inbox(&mut self, canonical_lines: &[u8]) -> Result<()> {
        self.append_unchecked(INBOX, canonical_lines)
    }
    /// Runtime already validated heard-state shape, monotonicity and provenance.
    pub fn host_replace_heard(&mut self, cbor: &[u8]) -> Result<()> {
        self.view.put(HEARD, cbor)
    }
    /// Runtime already validated and serialized the structured log import.
    pub fn host_append_log(&mut self, canonical_lines: &[u8]) -> Result<()> {
        self.append_unchecked(EVENTS, canonical_lines)
    }
    fn append_unchecked(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        let mut data = self
            .view
            .files
            .get(path)
            .map(|b| b.bytes.to_vec())
            .unwrap_or_default();
        data.extend_from_slice(bytes);
        self.view.put(path, &data)
    }
}
