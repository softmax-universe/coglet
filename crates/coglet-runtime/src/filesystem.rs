use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A filesystem seam independent of the eventual coglet-fs storage backend.
/// Snapshots use the ABI's exact path -> lowercase hex representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Snapshot(BTreeMap<String, String>);

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::new(BTreeMap::<String, String>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub fn valid_path(path: &str) -> bool {
    let mut components = path.split('/');
    components.next() == Some("")
        && matches!(components.next(), Some("code" | "data" | "logs"))
        && {
            let rest: Vec<_> = components.collect();
            !rest.is_empty()
                && rest
                    .iter()
                    .all(|c| !c.is_empty() && *c != "." && *c != "..")
        }
        && !path
            .chars()
            .any(|c| c <= '\u{1f}' || c == '\u{7f}' || c == '\\')
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn reserved_collision(path: &str) -> bool {
    [
        "/data/inbox.jsonl",
        crate::heard::PATH,
        "/logs/events.jsonl",
    ]
    .iter()
    .any(|reserved| {
        path == *reserved
            || path.starts_with(&format!("{reserved}/"))
            || reserved.starts_with(&format!("{path}/"))
    })
}

impl Snapshot {
    pub fn new(files: BTreeMap<String, String>) -> Result<Self> {
        for (path, bytes) in &files {
            if !valid_path(path) {
                return Err(format!("invalid path: {path:?}"));
            }
            if reserved_collision(path)
                && !matches!(
                    path.as_str(),
                    "/data/inbox.jsonl" | "/data/runtime/heard.cbor" | "/logs/events.jsonl"
                )
            {
                return Err("file/directory collision with reserved host file".into());
            }
            if bytes
                .bytes()
                .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
                || bytes.len() % 2 != 0
            {
                return Err("snapshot requires lowercase even-length hex".into());
            }
            // Regular files cannot be ancestors of other files.
            let mut prefix = path.as_str();
            while let Some((parent, _)) = prefix.rsplit_once('/') {
                if files.contains_key(parent) {
                    return Err("file/directory collision".into());
                }
                prefix = parent;
            }
        }
        crate::heard::decode(files.get(crate::heard::PATH).map(String::as_str))?;
        Ok(Self(files))
    }

    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.0
    }
    pub(crate) fn append_host_log(&mut self, line: &[u8]) {
        self.0
            .entry("/logs/events.jsonl".into())
            .or_default()
            .push_str(&hex::encode(line));
    }
    pub fn tree_hash(&self) -> Result<String> {
        coglet_fs::snapshot_tree_hash(&self.0).map_err(|e| e.to_string())
    }

    /// Replace a file from the guest's copy-on-write view. Append-only mounts
    /// require the old bytes as an exact prefix; descriptors cannot bypass this.
    pub fn guest_write(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        if !valid_path(path) {
            return Err("invalid filesystem path".into());
        }
        if path.starts_with("/code/") || reserved_collision(path) {
            return Err("permission_denied: host-owned file".into());
        }
        let encoded = hex::encode(bytes);
        if path.starts_with("/logs/")
            && self
                .0
                .get(path)
                .is_some_and(|old| !encoded.starts_with(old))
        {
            return Err("permission_denied: append-only logs".into());
        }
        let mut next = self.0.clone();
        next.insert(path.into(), encoded);
        *self = Self::new(next)?;
        Ok(())
    }
}
