use coglet_proto::quarantine::BundleSink;
use std::fs;

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
