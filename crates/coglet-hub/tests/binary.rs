use std::process::Command;
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
