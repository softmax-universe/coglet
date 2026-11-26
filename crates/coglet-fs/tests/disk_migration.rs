use coglet_fs::{DiskStore, Snapshot};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io, path::Path};

fn snapshot() -> Snapshot {
    Snapshot::from_files(BTreeMap::from([
        ("/code/step.wasm".into(), b"code".to_vec()),
        ("/data/nested/value".into(), vec![0, 255, 1]),
        ("/logs/events.jsonl".into(), b"{}\n".to_vec()),
    ]))
    .unwrap()
}

// Write the historical format directly, without using today's tree_hash/flush.
fn legacy_store(root: &Path, snapshot: &Snapshot) -> String {
    DiskStore::open(root).unwrap();
    let mut index = BTreeMap::new();
    for (path, bytes) in snapshot.files() {
        let blob = format!("{:x}", Sha256::digest(bytes));
        fs::write(root.join("blobs").join(&blob), bytes).unwrap();
        index.insert(path, blob);
    }
    let old = format!("{:x}", Sha256::digest(snapshot.cbor()));
    fs::write(
        root.join("trees").join(&old),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    fs::write(root.join("CURRENT"), &old).unwrap();
    old
}

#[test]
fn disk_migration_refuses_corruption_without_publishing() {
    for defect in ["name", "blob", "noncanonical", "target"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let snapshot = snapshot();
        let old = legacy_store(root, &snapshot);
        let new = snapshot.tree_hash();
        let mut current = old.clone();
        match defect {
            "name" => {
                current = "0".repeat(64);
                fs::rename(
                    root.join("trees").join(&old),
                    root.join("trees").join(&current),
                )
                .unwrap();
                fs::write(root.join("CURRENT"), &current).unwrap();
            }
            "blob" => {
                let blob = snapshot.blob_hash("/code/step.wasm").unwrap();
                fs::write(root.join("blobs").join(blob), b"corrupt").unwrap();
            }
            "noncanonical" => {
                let path = root.join("trees").join(&old);
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(b'\n');
                fs::write(path, bytes).unwrap();
            }
            "target" => fs::write(root.join("trees").join(&new), b"corrupt").unwrap(),
            _ => unreachable!(),
        }
        let err = DiskStore::open(root).unwrap().recover().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{defect}");
        let reason = match defect {
            "name" => "tree digest mismatch",
            "blob" => "blob digest mismatch",
            "noncanonical" => "noncanonical tree",
            "target" => "existing tree corrupt",
            _ => unreachable!(),
        };
        assert_eq!(err.to_string(), reason);
        assert_eq!(fs::read_to_string(root.join("CURRENT")).unwrap(), current);
        if defect == "target" {
            assert_eq!(fs::read(root.join("trees").join(&new)).unwrap(), b"corrupt");
        } else {
            assert!(!root.join("trees").join(&new).exists());
        }
    }
}

#[test]
fn disk_migration_retries_prepublished_tree_and_serializes_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let snapshot = snapshot();
    let old = legacy_store(root, &snapshot);
    let new = snapshot.tree_hash();
    // Model interruption after the new tree was published but before CURRENT.
    fs::copy(root.join("trees").join(&old), root.join("trees").join(&new)).unwrap();
    std::thread::scope(|scope| {
        let a = scope.spawn(|| DiskStore::open(root).unwrap().recover().unwrap().unwrap());
        let b = scope.spawn(|| DiskStore::open(root).unwrap().recover().unwrap().unwrap());
        assert_eq!(a.join().unwrap().export(), snapshot.export());
        assert_eq!(b.join().unwrap().export(), snapshot.export());
    });
    assert_eq!(fs::read_to_string(root.join("CURRENT")).unwrap(), new);
    // Losing pre-upgrade writers must not regain authority through the alias.
    assert_eq!(
        DiskStore::open(root)
            .unwrap()
            .flush(&snapshot, Some(&old))
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
}

#[test]
fn disk_store_upgrades_cbor_named_trees() {
    for snapshot in [Snapshot::default(), snapshot()] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let old = legacy_store(root, &snapshot);
        let new = snapshot.tree_hash();
        assert_ne!(old, new);
        let old_tree = fs::read(root.join("trees").join(&old)).unwrap();
        let store = DiskStore::open(root).unwrap();
        assert_eq!(
            store.recover().unwrap().unwrap().export(),
            snapshot.export()
        );
        assert_eq!(fs::read_to_string(root.join("CURRENT")).unwrap(), new);
        assert_eq!(fs::read(root.join("trees").join(&new)).unwrap(), old_tree);
        for (path, bytes) in snapshot.files() {
            let blob = format!("{:x}", Sha256::digest(bytes));
            assert_eq!(snapshot.blob_hash(path).unwrap(), blob);
            assert_eq!(fs::read(root.join("blobs").join(blob)).unwrap(), bytes);
        }
        // The legacy object is retained: interruption before CURRENT publication
        // must still have a complete old checkpoint to recover.
        assert_eq!(fs::read(root.join("trees").join(&old)).unwrap(), old_tree);
        for _ in 0..2 {
            let reopened = DiskStore::open(root).unwrap();
            assert_eq!(reopened.recover().unwrap().unwrap().tree_hash(), new);
            assert_eq!(fs::read_to_string(root.join("CURRENT")).unwrap(), new);
        }
        assert_eq!(
            store.flush(&snapshot, Some(&old)).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            store.flush(&snapshot, None).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let mut tx = snapshot.transaction();
        tx.write("/data/next", b"next").unwrap();
        let next = tx.commit();
        assert_eq!(store.flush(&next, Some(&new)).unwrap(), next.tree_hash());
        assert_eq!(store.recover().unwrap().unwrap().export(), next.export());
    }
}
