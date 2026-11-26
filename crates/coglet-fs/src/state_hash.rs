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
    Ok(tree_from_ids(
        snapshot
            .files()
            .map(|(path, bytes)| (path, object_id("blob", bytes))),
    ))
}

pub(crate) fn decoded_tree_hash(snapshot: &Snapshot) -> String {
    tree_from_ids(snapshot.files.iter().map(|(path, blob)| {
        let id = *blob.git_id.get_or_init(|| object_id("blob", &blob.bytes));
        (path.as_str(), id)
    }))
}

fn tree_from_ids<'a>(files: impl Iterator<Item = (&'a str, [u8; 32])>) -> String {
    // Bottom-up evaluation avoids a call stack proportional to path depth.
    type Entry<'a> = (&'a str, bool, [u8; 32]);
    let mut directories: BTreeMap<&str, Vec<Entry<'_>>> = BTreeMap::new();
    directories.insert("", Vec::new());
    for (path, id) in files {
        let (parent, name) = path.rsplit_once('/').expect("validated absolute path");
        directories
            .entry(parent)
            .or_default()
            .push((name, false, id));
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
            return hex(&id);
        }
        let (parent, name) = path.rsplit_once('/').expect("absolute directory");
        directories
            .get_mut(parent)
            .expect("parent directory")
            .push((name, true, id));
    }
    unreachable!("root directory always exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_blob_id_cache_is_lazy_and_shared_across_copy_on_write() {
        let original = Snapshot::from_files(BTreeMap::from([
            ("/code/bin/step.wasm".into(), vec![23; 65536]),
            ("/data/state".into(), b"old".to_vec()),
        ]))
        .unwrap();
        let blob = &original.files["/code/bin/step.wasm"];
        assert!(blob.git_id.get().is_none());
        let expected = snapshot_tree_hash(&original.export()).unwrap();
        assert_eq!(original.tree_hash(), expected);
        assert_eq!(blob.git_id.get(), Some(&object_id("blob", &[23; 65536])));
        let mut changed = original.clone();
        changed.put("/data/state", b"new").unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &blob.git_id,
            &changed.files["/code/bin/step.wasm"].git_id
        ));
        assert_ne!(changed.tree_hash(), expected);
        assert_eq!(
            changed.tree_hash(),
            snapshot_tree_hash(&changed.export()).unwrap()
        );
        assert_eq!(original.tree_hash(), expected);
        changed.put("/code/bin/step.wasm", b"replacement").unwrap();
        assert!(!std::sync::Arc::ptr_eq(
            &blob.git_id,
            &changed.files["/code/bin/step.wasm"].git_id
        ));
        assert_eq!(
            changed.tree_hash(),
            snapshot_tree_hash(&changed.export()).unwrap()
        );
    }

    #[test]
    fn cached_git_tree_matches_hex_reference_for_prefix_order_and_empty_blobs() {
        let snapshot = Snapshot::from_files(BTreeMap::from([
            ("/data/foo.bar".into(), vec![]),
            ("/data/foo/child".into(), b"nested".to_vec()),
            ("/data/foo0".into(), vec![0, 255]),
            ("/logs/é".into(), vec![1]),
        ]))
        .unwrap();
        assert_eq!(
            snapshot.tree_hash(),
            snapshot_tree_hash(&snapshot.export()).unwrap()
        );
        assert_eq!(
            Snapshot::default().tree_hash(),
            snapshot_tree_hash(&BTreeMap::new()).unwrap()
        );
        assert_eq!(
            snapshot.blob_hash("/data/foo0").unwrap(),
            crate::digest(&[0, 255])
        );
    }
}
