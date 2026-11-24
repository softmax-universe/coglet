//! Explicit privileged Linux acceptance, not silently skipped portability proof.
#![cfg(target_os = "linux")]
use coglet_hub::{
    Kernel,
    operator::{
        server,
        service::{Provisioning, Service},
        socket::{OperatorSocket, StateLock},
    },
};
use serde_json::json;
use std::{
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::Arc,
    time::Duration,
};
#[test]
#[ignore = "subprocess helper; selected only by operator_peer_uid_nonowner_subprocess"]
fn operator_peer_uid_root_child() {
    let path = std::env::var_os("COGLET_OPERATOR_SOCKET").expect("explicit child socket");
    assert_eq!(
        std::fs::metadata("/proc/self").unwrap().uid(),
        0,
        "negative control must really be root"
    );
    let mut s = std::os::unix::net::UnixStream::connect(path).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let request=serde_json::to_vec(&json!({"version":"local-1","operation_id":"11".repeat(16),"op":"enrollment.window","timeout_ms":1000,"params":{"duration_ms":1000}})).unwrap();
    // Root bypasses directory/socket DAC, so EOF/reset proves credential rejection.
    let _ = s.write_all(&(request.len() as u32).to_be_bytes());
    let _ = s.write_all(&request);
    let mut byte = [0];
    match s.read(&mut byte) {
        Ok(0) => (),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ) => {}
        other => panic!("non-owner was not rejected by credentials: {other:?}"),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires passwordless sudo to exercise root peer; run explicitly"]
async fn operator_peer_uid_nonowner_subprocess() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let uid = std::fs::metadata(dir.path()).unwrap().uid();
    assert_ne!(uid, 0, "run service as ordinary user");
    let lock = StateLock::acquire(dir.path()).unwrap();
    let kernel = Kernel::open(
        dir.path(),
        iroh::SecretKey::generate().public(),
        Arc::new(|| Some(10)),
    )
    .unwrap();
    let p:Provisioning=serde_json::from_value(json!({"uid":uid,"actor":format!("coglet:{}","11".repeat(32)),"operations":["enrollment.window"],"max_window_ms":1000,"grants":[]})).unwrap();
    let service = Arc::new(Service::new(kernel.clone(), uid, p).unwrap());
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let (stop, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server::serve(socket, service, 8, async {
        let _ = rx.await;
    }));
    let path = dir.path().join("operator.sock");
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new("sudo")
        .args(["-n", "env"])
        .arg(format!("COGLET_OPERATOR_SOCKET={}", path.display()))
        .arg(exe)
        .args([
            "--ignored",
            "--exact",
            "operator_peer_uid_root_child",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "root subprocess failed: {:?} {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        kernel.audit().await.unwrap().is_empty(),
        "nonowner reached admission"
    );
    // Same request under service UID must succeed, ruling out parser/listener failure.
    let response=tokio::task::spawn_blocking(move || {
        let mut s=std::os::unix::net::UnixStream::connect(path).unwrap();s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let r=serde_json::to_vec(&json!({"version":"local-1","operation_id":"11".repeat(16),"op":"enrollment.window","timeout_ms":1000,"params":{"duration_ms":1000}})).unwrap();
        s.write_all(&(r.len() as u32).to_be_bytes()).unwrap();s.write_all(&r).unwrap();let mut n=[0;4];s.read_exact(&mut n).unwrap();let mut b=vec![0;u32::from_be_bytes(n) as usize];s.read_exact(&mut b).unwrap();serde_json::from_slice::<serde_json::Value>(&b).unwrap()
    }).await.unwrap();
    assert_eq!(response["receipt"]["state"], "applied");
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
