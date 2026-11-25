//! Deliberately synthetic transport fault peers; not deployment acceptance.
#![cfg(unix)]
use serde_json::{Value, json};
use std::os::unix::fs::{PermissionsExt, symlink};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

#[tokio::test]
async fn cli_operator_queued_failed_and_malformed_never_report_success() {
    for state in [
        "queued",
        "failed",
        "cancelled",
        "oversized",
        "duplicate",
        "truncated",
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("operator.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let n = stream.read_u32().await.unwrap();
            let mut raw = vec![0; n as usize];
            stream.read_exact(&mut raw).await.unwrap();
            let r: Value = serde_json::from_slice(&raw).unwrap();
            if state == "oversized" {
                stream.write_u32(1048577).await.unwrap();
                return;
            }
            if state == "truncated" {
                stream.write_u32(100).await.unwrap();
                stream.write_all(b"{}").await.unwrap();
                return;
            }
            let error = if state == "queued" {
                Value::Null
            } else {
                json!({"code":"permission_denied","message":"fixture refusal","retryable":false})
            };
            let value = json!({"version":"local-1","operation_id":r["operation_id"],"op":r["op"],"receipt":{"operation_id":r["operation_id"],"state":if state=="duplicate"{"applied"}else{state},"step":if state=="duplicate"{json!(1)}else{Value::Null},"error":if state=="duplicate"{Value::Null}else{error}},"result":null,"error":null});
            let mut raw = serde_json::to_string(&value).unwrap();
            if state == "duplicate" {
                raw = raw.replace(
                    "\"state\":\"applied\"",
                    "\"state\":\"failed\",\"state\":\"applied\"",
                );
            }
            stream.write_u32(raw.len() as u32).await.unwrap();
            stream.write_all(raw.as_bytes()).await.unwrap();
        });
        let out = tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_coglet"))
                .env("TOKIO_WORKER_THREADS", "2")
                .args([
                    "stop",
                    "--operator-socket",
                    path.to_str().unwrap(),
                    "--operation-id",
                    &"11".repeat(16),
                    "--device",
                    &iroh::SecretKey::generate().public().to_string(),
                    "--target",
                    &format!("coglet:{}", iroh::SecretKey::generate().public()),
                ])
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(!out.status.success(), "{state}");
        if state == "queued" {
            assert_eq!(out.status.code(), Some(3));
            let r: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(r["receipt"]["state"], "queued");
        }
        server.await.unwrap();
    }
}
#[tokio::test]
async fn operator_client_refuses_symlink_and_public_socket_directory() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("operator.sock");
    let _listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = root.path().join("alias");
    symlink(&path, &alias).unwrap();
    let req=coglet_cli::operator::request("stop",&"11".repeat(16),1000,json!({"device":iroh::SecretKey::generate().public().to_string(),"target":format!("coglet:{}",iroh::SecretKey::generate().public())})).unwrap();
    assert!(coglet_cli::operator::call(&alias, &req).await.is_err());
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(coglet_cli::operator::call(&path, &req).await.is_err());
}
