use coglet_fs::{EVENTS, Error, GitStore, HEARD, INBOX, Snapshot};
use std::process::Command;

#[test]
fn reserved_ancestor_and_descendant_aliases_cannot_wedge_host_boundaries() {
    for path in [
        "/data/runtime",
        "/data/runtime/heard.cbor/x",
        "/data/inbox.jsonl/x",
        "/logs/events.jsonl/x",
    ] {
        let mut tx = Snapshot::default().transaction();
        tx.write("/data/source", b"source").unwrap();
        let hash = tx.snapshot().tree_hash();
        assert_eq!(
            tx.create(path),
            Err(Error::PermissionDenied),
            "create {path}"
        );
        assert_eq!(
            tx.append(path, b"bad"),
            Err(Error::PermissionDenied),
            "append {path}"
        );
        assert_eq!(
            tx.write(path, b"bad"),
            Err(Error::PermissionDenied),
            "write {path}"
        );
        assert_eq!(
            tx.rename("/data/source", path),
            Err(Error::PermissionDenied),
            "rename {path}"
        );
        assert_eq!(
            tx.debug_write(path, b"bad"),
            Err(Error::PermissionDenied),
            "debug {path}"
        );
        assert_eq!(tx.snapshot().tree_hash(), hash);
        tx.host_append_inbox(b"inbox\n").unwrap();
        tx.host_replace_heard(b"heard").unwrap();
        tx.host_append_log(b"log\n").unwrap();
        assert_eq!(tx.snapshot().read(INBOX).unwrap(), b"inbox\n");
        assert_eq!(tx.snapshot().read(HEARD).unwrap(), b"heard");
        assert_eq!(tx.snapshot().read(EVENTS).unwrap(), b"log\n");
    }
    let mut tx = Snapshot::default().transaction();
    tx.write("/data/runtime/other", b"allowed").unwrap();
    tx.host_replace_heard(b"heard").unwrap();
}

#[test]
fn unallocatable_truncate_returns_resource_error_without_mutation() {
    let mut tx = Snapshot::default().transaction();
    tx.write("/data/file", b"old").unwrap();
    let hash = tx.snapshot().tree_hash();
    assert_eq!(
        tx.truncate("/data/file", usize::MAX),
        Err(Error::ResourceExhausted)
    );
    assert_eq!(tx.snapshot().tree_hash(), hash);
}

#[test]
fn git_store_rejects_uninitialized_subdirectory_without_writing_parent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let sub = dir.path().join("not-a-repo");
    std::fs::create_dir(&sub).unwrap();
    assert!(
        GitStore::new(&sub)
            .flush_logs(&Snapshot::default(), None)
            .is_err()
    );
    assert!(GitStore::new(dir.path()).logs_head().unwrap().is_none());
    assert!(
        std::fs::read_dir(dir.path().join(".git/objects"))
            .unwrap()
            .all(|e| {
                let name = e.unwrap().file_name();
                name == "info" || name == "pack"
            })
    );
}

#[test]
fn git_incompatible_log_name_is_rejected_before_snapshot_or_projection() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--bare", "--quiet"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let mut tx = Snapshot::default().transaction();
    let before = tx.snapshot().tree_hash();
    // ADR-0035 rejects these at ABI admission, before the logs-only GitStore
    // guard is reachable. Refusal must leave the snapshot and ref untouched.
    for path in [
        "/logs/.git/config",
        "/logs/.Git/config",
        "/logs/nested/.GIT/x",
    ] {
        assert_eq!(
            tx.append(path, b"invalid guest path"),
            Err(Error::PermissionDenied)
        );
        assert_eq!(tx.snapshot().tree_hash(), before);
        assert!(GitStore::new(dir.path()).logs_head().unwrap().is_none());
    }
    tx.append("/logs/git/config", b"valid guest path").unwrap();
    GitStore::new(dir.path())
        .flush_logs(&tx.commit(), None)
        .unwrap();
    assert!(GitStore::new(dir.path()).logs_head().unwrap().is_some());
}
