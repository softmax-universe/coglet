#![cfg(unix)]
use serde_json::json;
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};
#[test]
fn operator_startup_socket_uses_explicit_service_actor() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let uid = std::fs::metadata(dir.path()).unwrap().uid();
    let cfg = dir.path().join("config.json");
    std::fs::write(&cfg,serde_json::to_vec(&json!({"operator":{"uid":uid,"actor":format!("coglet:{}","11".repeat(32)),"operations":["enrollment.window"],"max_window_ms":1000,"grants":[]}})).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_coglet-hub"))
        .args(["serve", dir.path().to_str().unwrap(), cfg.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let exists = dir.path().join("operator.sock").exists();
    let status = child.try_wait().unwrap();
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    assert!(
        exists && status.is_none(),
        "startup did not expose local socket: stdout={line:?} stderr={:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}
