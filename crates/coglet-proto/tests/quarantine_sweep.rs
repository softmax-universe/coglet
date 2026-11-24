use coglet_proto::quarantine::BundleSink;
use std::fs;

#[tokio::test]
async fn drop_serializes_with_startup_sweep_gate() {
    use std::{sync::mpsc, time::Duration};
    let root = std::env::temp_dir().join(format!(
        "coglet-drop-gate-{}",
        iroh::SecretKey::generate().public()
    ));
    fs::create_dir(&root).unwrap();
    let sink = BundleSink::create(&root, 10).await.unwrap();
    let gate = fs::File::open(&root).unwrap();
    gate.lock().unwrap();
    let (started_tx, started) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let dropping = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        drop(sink);
        done_tx.send(()).unwrap();
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let premature = done.recv_timeout(Duration::from_millis(100)).is_ok();
    let remaining = fs::read_dir(&root).unwrap().count();
    gate.unlock().unwrap();
    dropping.join().unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert!(!premature, "Drop raced a sweep that owns the parent gate");
    assert_eq!(
        remaining, 1,
        "custody must remain while the sweep gate is owned"
    );
}

#[tokio::test]
async fn startup_sweep_removes_only_unlocked_owned_orphans() {
    let root = std::env::temp_dir().join(format!(
        "coglet-sweep-{}",
        iroh::SecretKey::generate().public()
    ));
    fs::create_dir(&root).unwrap();
    let live = BundleSink::create(&root, 10).await.unwrap();
    let orphan = root.join(format!("git-{}", "a".repeat(64)));
    fs::create_dir(&orphan).unwrap();
    fs::write(orphan.join("lease"), []).unwrap();
    fs::write(orphan.join("bundle"), b"unfinished").unwrap();
    fs::create_dir(root.join("unrelated")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        root.join("unrelated"),
        root.join(format!("git-{}", "b".repeat(64))),
    )
    .unwrap();
    assert_eq!(BundleSink::sweep_orphans(&root).await.unwrap(), 1);
    assert!(!orphan.exists());
    assert!(root.join("unrelated").exists());
    assert_eq!(BundleSink::sweep_orphans(&root).await.unwrap(), 0);
    drop(live);
    fs::remove_dir_all(root).unwrap();
}
