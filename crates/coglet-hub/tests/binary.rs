use std::process::Command;
#[test]
fn dangling_identity_symlink_is_rejected_without_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("hub.key");
    let absent = dir.path().join("missing");
    std::os::unix::fs::symlink(&absent, &key).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_coglet-hub"))
        .args(["identity", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "dangling key symlink was silently replaced"
    );
    assert_eq!(std::fs::read_link(key).unwrap(), absent);
    assert!(result.stdout.is_empty());
}
#[test]
fn binary_initializes_private_stable_identity() {
    let dir = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_coglet-hub");
    let first = Command::new(binary)
        .args(["identity", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = Command::new(binary)
        .args(["identity", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(first.stdout, second.stdout);
    let identity: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(identity["hub"].as_str().unwrap().len(), 64);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(dir.path().join("hub.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    // Invalid commands never print private keys or generate a public server.
    let bad = Command::new(binary).arg("unknown").output().unwrap();
    assert!(!bad.status.success());
}
