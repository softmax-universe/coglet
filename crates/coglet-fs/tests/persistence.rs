use coglet_fs::{DiskStore, GitStore, Snapshot};
use std::{collections::BTreeMap, fs, path::Path, process::Command};
fn snapshot(new: bool) -> Snapshot {
    let word = if new { b"new" } else { b"old" };
    Snapshot::from_files(BTreeMap::from([
        ("/code/step.wasm".into(), b"code".to_vec()),
        ("/data/a".into(), [word.as_slice(), b" private A"].concat()),
        ("/data/b".into(), [word.as_slice(), b" private B"].concat()),
        (
            "/logs/a".into(),
            [
                b"old log A\n".as_slice(),
                if new { b"new log A\n" } else { b"" },
            ]
            .concat(),
        ),
        (
            "/logs/nested/b".into(),
            [
                b"old log B\n".as_slice(),
                if new { b"new log B\n" } else { b"" },
            ]
            .concat(),
        ),
    ]))
    .unwrap()
}
fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn init_git(repo: &Path) {
    git(repo, &["init", "--bare", "--quiet"]);
}
fn child(kind: &str, path: &Path, stage: usize) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_worker", "--ignored", "--nocapture"])
        .env("COGLET_FS_CRASH_KIND", kind)
        .env("COGLET_FS_CRASH_PATH", path)
        .env("COGLET_FS_CRASH_STAGE", stage.to_string())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(86),
        "child never reached crash stage {stage}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
#[ignore = "subprocess target; invoked by crash matrix parents"]
fn crash_worker() {
    let kind = std::env::var("COGLET_FS_CRASH_KIND").unwrap();
    let path = std::env::var("COGLET_FS_CRASH_PATH").unwrap();
    let stage: usize = std::env::var("COGLET_FS_CRASH_STAGE")
        .unwrap()
        .parse()
        .unwrap();
    let mut seen = 0;
    let mut crash = || {
        seen += 1;
        if seen == stage {
            std::process::exit(86);
        }
        Ok(())
    };
    match kind.as_str() {
        "disk" => {
            DiskStore::open(&path)
                .unwrap()
                .flush_with(&snapshot(true), Some(&snapshot(false).tree_hash()), |_| {
                    crash()
                })
                .unwrap();
        }
        "initial" => {
            DiskStore::open(&path)
                .unwrap()
                .flush_with(&snapshot(true), None, |_| crash())
                .unwrap();
        }
        "git" => {
            let store = GitStore::new(&path);
            let old = store.logs_head().unwrap();
            store
                .flush_logs_with(&snapshot(true), old.as_deref(), |_| crash())
                .unwrap();
        }
        "hash" => {
            println!("TREE_HASH={}", snapshot(true).tree_hash());
            std::process::exit(86);
        }
        _ => panic!("unknown worker mode"),
    }
    panic!("crash point not reached");
}
#[test]
fn stable_hash_across_independent_processes_and_insertion_order() {
    for _ in 0..3 {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_worker", "--ignored", "--nocapture"])
            .env("COGLET_FS_CRASH_KIND", "hash")
            .env("COGLET_FS_CRASH_PATH", "unused")
            .env("COGLET_FS_CRASH_STAGE", "0")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(86));
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("TREE_HASH={}", snapshot(true).tree_hash()))
        );
    }
    let mut tx = Snapshot::default().transaction();
    let entries: Vec<_> = snapshot(true)
        .files()
        .map(|(p, b)| (p.to_owned(), b.to_vec()))
        .collect();
    // All mounts are trusted bootstrap data; reconstruct reverse insertion via map.
    let reverse = Snapshot::from_files(entries.into_iter().rev().collect()).unwrap();
    assert_eq!(reverse.tree_hash(), snapshot(true).tree_hash());
    tx.write("/data/different", b"x").unwrap();
    assert_ne!(tx.snapshot().tree_hash(), reverse.tree_hash());
}
#[test]
fn disk_crash_at_every_flush_point_recovers_old_or_new_never_mixed() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskStore::open(dir.path()).unwrap();
    let old = snapshot(false);
    let new = snapshot(true);
    store.flush(&old, None).unwrap();
    let mut points = Vec::new();
    store
        .flush_with(&new, Some(&old.tree_hash()), |p| {
            points.push(p);
            Ok(())
        })
        .unwrap();
    for stage in 1..=points.len() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskStore::open(dir.path()).unwrap();
        store.flush(&old, None).unwrap();
        child("disk", dir.path(), stage);
        let recovered = store.recover().unwrap().unwrap();
        assert!(
            recovered.export() == old.export() || recovered.export() == new.export(),
            "mixed tree at {stage}: {:?}",
            points[stage - 1]
        );
        // Orphans never block a subsequent checkpoint/restart.
        store.flush(&new, Some(&recovered.tree_hash())).unwrap();
        assert_eq!(store.recover().unwrap().unwrap().export(), new.export());
    }
    println!(
        "disk replacement process-crash points: {} {points:?}",
        points.len()
    );
}
#[test]
fn disk_first_checkpoint_crashes_recover_none_or_complete_new_tree() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskStore::open(dir.path()).unwrap();
    let new = snapshot(true);
    let mut count = 0;
    store
        .flush_with(&new, None, |_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    for stage in 1..=count {
        let dir = tempfile::tempdir().unwrap();
        DiskStore::open(dir.path()).unwrap();
        child("initial", dir.path(), stage);
        if let Some(recovered) = DiskStore::open(dir.path()).unwrap().recover().unwrap() {
            assert_eq!(recovered.export(), new.export());
        }
    }
    println!("disk initial process-crash points: {count}");
}
#[test]
fn disk_conflicts_corruption_and_deduplicated_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskStore::open(dir.path()).unwrap();
    assert!(store.recover().unwrap().is_none());
    let old = snapshot(false);
    let hash = store.flush(&old, None).unwrap();
    assert!(store.flush(&snapshot(true), None).is_err());
    assert_eq!(store.recover().unwrap().unwrap().tree_hash(), hash);
    store.flush(&old, Some(&hash)).unwrap();
    assert_eq!(fs::read_dir(dir.path().join("blobs")).unwrap().count(), 5);
    fs::write(
        dir.path()
            .join("blobs")
            .join(old.blob_hash("/data/a").unwrap()),
        b"corrupt",
    )
    .unwrap();
    assert!(store.recover().is_err());
    assert!(store.flush(&old, Some(&hash)).is_err());
}
#[test]
fn disk_rejects_corrupt_tree_pointer_and_missing_blob() {
    for target in ["tree", "pointer", "missing"] {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskStore::open(dir.path()).unwrap();
        let old = snapshot(false);
        let hash = store.flush(&old, None).unwrap();
        match target {
            "tree" => fs::write(dir.path().join("trees").join(&hash), b"{}").unwrap(),
            "pointer" => fs::write(dir.path().join("CURRENT"), b"../../escape").unwrap(),
            _ => fs::remove_file(
                dir.path()
                    .join("blobs")
                    .join(old.blob_hash("/data/a").unwrap()),
            )
            .unwrap(),
        }
        assert!(store.recover().is_err(), "{target}");
    }
}
#[test]
fn disk_io_failure_does_not_publish_new_tree() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskStore::open(dir.path()).unwrap();
    let old = snapshot(false);
    store.flush(&old, None).unwrap();
    assert!(
        store
            .flush_with(&snapshot(true), Some(&old.tree_hash()), |_| Err(
                std::io::Error::other("injected ENOSPC")
            ))
            .is_err()
    );
    assert_eq!(store.recover().unwrap().unwrap().export(), old.export());
}
#[test]
fn git_crash_at_every_object_and_ref_point_selects_complete_old_or_new() {
    let dir = tempfile::tempdir().unwrap();
    init_git(dir.path());
    let store = GitStore::new(dir.path());
    let old = store.flush_logs(&snapshot(false), None).unwrap();
    let mut points = Vec::new();
    let new = store
        .flush_logs_with(&snapshot(true), Some(&old), |p| {
            points.push(p);
            Ok(())
        })
        .unwrap();
    for stage in 1..=points.len() {
        let dir = tempfile::tempdir().unwrap();
        init_git(dir.path());
        let store = GitStore::new(dir.path());
        assert_eq!(store.flush_logs(&snapshot(false), None).unwrap(), old);
        child("git", dir.path(), stage);
        let head = store.logs_head().unwrap().unwrap();
        assert!(head == old || head == new, "unexpected ref at {stage}");
        for (path, old_bytes) in snapshot(false)
            .files()
            .filter(|(p, _)| p.starts_with("/logs/"))
        {
            let bytes = if head == old {
                old_bytes.to_vec()
            } else {
                snapshot(true).read(path).unwrap().to_vec()
            };
            let expected = String::from_utf8(bytes).unwrap().trim().to_owned();
            assert_eq!(
                git(
                    dir.path(),
                    &[
                        "show",
                        &format!("{head}:{}", path.strip_prefix("/logs/").unwrap())
                    ]
                ),
                expected
            );
        }
        git(dir.path(), &["fsck", "--full"]);
    }
    println!("git process-crash points: {} {points:?}", points.len());
}
#[test]
fn git_never_writes_private_or_code_objects_and_preserves_nested_names() {
    let dir = tempfile::tempdir().unwrap();
    init_git(dir.path());
    let store = GitStore::new(dir.path());
    let snapshot = snapshot(true);
    let commit = store.flush_logs(&snapshot, None).unwrap();
    assert_eq!(
        git(dir.path(), &["ls-tree", "-r", "--name-only", &commit]),
        "a\nnested/b"
    );
    let objects = git(
        dir.path(),
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype)",
        ],
    );
    let blobs: Vec<_> = objects.lines().filter(|l| l.ends_with(" blob")).collect();
    assert_eq!(blobs.len(), 2);
    for blob in blobs {
        let id = blob.split_whitespace().next().unwrap();
        assert!(git(dir.path(), &["cat-file", "blob", id]).starts_with("old log"));
    }
    assert!(store.flush_logs(&snapshot, None).is_err());
    assert_eq!(store.logs_head().unwrap().unwrap(), commit);
    assert!(store.flush_logs(&snapshot, Some("--malformed")).is_err());
}
#[test]
fn git_empty_logs_tree_and_same_content_are_deterministic() {
    let mut commits = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        init_git(dir.path());
        commits.push(
            GitStore::new(dir.path())
                .flush_logs(&Snapshot::default(), None)
                .unwrap(),
        );
    }
    assert_eq!(commits[0], commits[1]);
}

#[test]
fn git_symbolic_logs_ref_cannot_modify_an_unrelated_branch() {
    let dir = tempfile::tempdir().unwrap();
    init_git(dir.path());
    let store = GitStore::new(dir.path());
    let old = store.flush_logs(&snapshot(false), None).unwrap();
    git(dir.path(), &["update-ref", "refs/heads/unrelated", &old]);
    git(
        dir.path(),
        &["symbolic-ref", "refs/heads/logs", "refs/heads/unrelated"],
    );
    assert!(store.logs_head().is_err());
    assert!(store.flush_logs(&snapshot(true), Some(&old)).is_err());
    assert_eq!(git(dir.path(), &["rev-parse", "refs/heads/unrelated"]), old);
}

#[test]
fn concurrent_disk_writers_have_exactly_one_cas_winner() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskStore::open(dir.path()).unwrap();
    let old = snapshot(false);
    store.flush(&old, None).unwrap();
    let mut threads = Vec::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    for i in 0..4 {
        let path = dir.path().to_owned();
        let old = old.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut tx = old.transaction();
            tx.write("/data/winner", &[i]).unwrap();
            let store = DiskStore::open(path).unwrap();
            barrier.wait();
            store.flush(&tx.commit(), Some(&old.tree_hash())).is_ok()
        }));
    }
    let wins = threads
        .into_iter()
        .map(|t| usize::from(t.join().unwrap()))
        .sum::<usize>();
    assert_eq!(wins, 1);
    assert!(
        store
            .recover()
            .unwrap()
            .unwrap()
            .read("/data/winner")
            .is_ok()
    );
}
