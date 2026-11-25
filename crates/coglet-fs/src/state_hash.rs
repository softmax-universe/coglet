//! ABI filesystem identity (ADR-0035), independent of the logs-only GitStore.
use crate::{Result, Snapshot, hex, unhex};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn object_id(kind: &str, bytes: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(format!("{kind} {}\0", bytes.len()));
    hash.update(bytes);
    hash.finalize().into()
}

/// Git SHA-256 tree object id of an ABI snapshot (absolute paths to lowercase
/// hex file contents). Only regular 100644 blobs and implicit 40000 trees exist.
/// Invalid paths, hex or file/directory collisions are rejected, not hashed.
/// No Git executable or repository is involved.
pub fn snapshot_tree_hash(files: &BTreeMap<String, String>) -> Result<String> {
    let decoded = files
        .iter()
        .map(|(path, bytes)| {
            Ok((
                path.clone(),
                unhex(bytes).ok_or(crate::Error::InvalidArgument)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let snapshot = Snapshot::from_files(decoded)?;
    // Bottom-up evaluation avoids a call stack proportional to path depth.
    type Entry<'a> = (&'a str, bool, [u8; 32]);
    let mut directories: BTreeMap<&str, Vec<Entry<'_>>> = BTreeMap::new();
    directories.insert("", Vec::new());
    for (path, bytes) in snapshot.files() {
        let (parent, name) = path.rsplit_once('/').expect("validated absolute path");
        directories
            .entry(parent)
            .or_default()
            .push((name, false, object_id("blob", bytes)));
        let mut directory = parent;
        while !directory.is_empty() {
            directories.entry(directory).or_default();
            directory = directory.rsplit_once('/').expect("absolute directory").0;
        }
    }
    while let Some((path, mut entries)) = directories.pop_last() {
        // Git compares tree names as though suffixed by '/', not NUL. In
        // particular foo.bar sorts before tree foo, which sorts before foo0.
        entries.sort_by_cached_key(|(name, tree, _)| {
            let mut key = name.as_bytes().to_vec();
            if *tree {
                key.push(b'/');
            }
            key
        });
        let mut body = Vec::new();
        for (name, tree, id) in entries {
            body.extend_from_slice(if tree { b"40000 " } else { b"100644 " });
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(&id);
        }
        let id = object_id("tree", &body);
        if path.is_empty() {
            return Ok(hex(&id));
        }
        let (parent, name) = path.rsplit_once('/').expect("absolute directory");
        directories
            .get_mut(parent)
            .expect("parent directory")
            .push((name, true, id));
    }
    unreachable!("root directory always exists")
}
