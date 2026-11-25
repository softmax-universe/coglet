#![cfg(unix)]
use coglet_hub::{
    Kernel,
    operator::{
        server,
        service::{Provisioning, Service},
        socket::{OperatorSocket, StateLock},
    },
};
use coglet_proto::transport::{OperationHandler, RequestContext};
use iroh::SecretKey;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn call(path: &std::path::Path, request: &Value) -> Value {
    let mut stream = tokio::net::UnixStream::connect(path).await.unwrap();
    let bytes = serde_json::to_vec(request).unwrap();
    stream.write_u32(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
    let len = stream.read_u32().await.unwrap();
    let mut bytes = vec![0; len as usize];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
async fn start(
    root: &std::path::Path,
    hub: iroh::EndpointId,
    device: iroh::EndpointId,
) -> (
    Kernel,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let uid = std::fs::metadata(root).unwrap().uid();
    let lock = StateLock::acquire(root).unwrap();
    let kernel = Kernel::open(root, hub, Arc::new(|| Some(10))).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let service = Arc::new(Service::new(kernel.clone(), uid, provisioning(uid, device)).unwrap());
    let (stop, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server::serve(socket, service, 8, async {
        let _ = rx.await;
    }));
    (kernel, stop, task)
}
async fn enroll(
    kernel: &Kernel,
    hub: iroh::EndpointId,
    device: iroh::EndpointId,
    secret: &str,
) -> Value {
    kernel.handle(RequestContext {peer:device,local:hub,deadline:tokio::time::Instant::now()+Duration::from_secs(5)},json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"22".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":secret}})).await
}
#[tokio::test]
async fn operator_invite_hash_only_restart_and_window_retry_stay_closed() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let path = dir.path().join("operator.sock");
    let (kernel, stop, task) = start(dir.path(), hub, device).await;
    let window = req(3, "enrollment.window", json!({"duration_ms":60000}));
    let applied = call(&path, &window).await;
    assert_eq!(applied["receipt"]["state"], "applied", "{applied}");
    let secret = "55".repeat(32);
    let hash = hex::encode(Sha256::digest(secret.as_bytes()));
    let invite = req(
        4,
        "enrollment.invite",
        json!({"window_id":window["operation_id"],"expected_device":device.to_string(),"secret_sha256":hash,"ttl_ms":60000,"grants":[]}),
    );
    let invited = call(&path, &invite).await;
    assert_eq!(invited["receipt"]["state"], "applied", "{invited}");
    let duplicate = req(8, "enrollment.invite", invite["params"].clone());
    assert_eq!(
        call(&path, &duplicate).await["receipt"]["error"]["code"],
        "permission_denied",
        "hash reuse must be terminal refusal, not queued storage failure"
    );
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    let persisted: String = db
        .query_row("SELECT hash FROM proto_invites", [], |r| r.get(0))
        .unwrap();
    assert_eq!(persisted, hash);
    assert!(!format!("{:?}", kernel.audit().await.unwrap()).contains(&secret));
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    drop(kernel);
    let (kernel, stop, task) = start(dir.path(), hub, device).await;
    assert_eq!(call(&path, &window).await, applied);
    assert_eq!(call(&path, &invite).await, invited);
    assert_eq!(
        enroll(&kernel, hub, device, &secret).await["error"]["code"],
        "unauthenticated"
    );
    let fresh = req(5, "enrollment.invite", invite["params"].clone());
    assert_eq!(
        call(&path, &fresh).await["receipt"]["error"]["code"],
        "unauthenticated"
    );
    let new_window = req(6, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(
        call(&path, &new_window).await["receipt"]["state"],
        "applied"
    );
    let fresh_secret = "66".repeat(32);
    let fresh_invite = req(
        7,
        "enrollment.invite",
        json!({"window_id":new_window["operation_id"],"expected_device":device.to_string(),"secret_sha256":hex::encode(Sha256::digest(fresh_secret.as_bytes())),"ttl_ms":60000,"grants":[]}),
    );
    assert_eq!(
        call(&path, &fresh_invite).await["receipt"]["state"],
        "applied"
    );
    assert_eq!(
        enroll(&kernel, hub, device, &secret).await["error"]["code"],
        "unauthenticated"
    );
    assert_eq!(
        enroll(&kernel, hub, device, &fresh_secret).await["type"],
        "reply"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
fn req(id: u8, op: &str, params: Value) -> Value {
    json!({"version":"local-1","operation_id":format!("{id:032x}"),"op":op,"timeout_ms":1000,"params":params})
}
#[tokio::test]
async fn operator_socket_revoke_audit_rollback_and_generation_tombstone() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let (kernel, stop, task) = start(dir.path(), hub, device).await;
    let path = dir.path().join("operator.sock");
    let window = req(10, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    let secret = "77".repeat(32);
    let invite = req(
        11,
        "enrollment.invite",
        json!({"window_id":window["operation_id"],"expected_device":device.to_string(),"secret_sha256":hex::encode(Sha256::digest(secret.as_bytes())),"ttl_ms":60000,"grants":[{"peer":device.to_string(),"method":"hub.status","target":null,"generation":1,"expires_unix_s":100}]}),
    );
    assert_eq!(call(&path, &invite).await["receipt"]["state"], "applied");
    assert_eq!(enroll(&kernel, hub, device, &secret).await["type"], "reply");
    let authority = coglet_proto::authority::Authority::open(dir.path().join("kernel.db")).unwrap();
    let revoke = req(
        12,
        "grant.revoke",
        json!({"peer":device.to_string(),"method":"hub.status","target":null,"generation":2}),
    );
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_finish BEFORE INSERT ON audit WHEN json_extract(NEW.record,'$.method')='operator.finish' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let response = call(&path, &revoke).await;
    assert_ne!(response["receipt"]["state"], "applied");
    authority
        .authorize(device, "hub.status", None, Some(10), 1)
        .unwrap();
    db.execute_batch("DROP TRIGGER fail_finish").unwrap();
    let applied = call(&path, &revoke).await;
    assert_eq!(applied["receipt"]["state"], "applied", "{applied}");
    assert!(
        authority
            .authorize(device, "hub.status", None, Some(10), 1)
            .is_err()
    );
    assert!(
        authority
            .authorize(device, "hub.status", None, Some(10), 2)
            .is_err()
    );
    assert_eq!(call(&path, &revoke).await, applied);
    let rollback = req(
        13,
        "grant.provision",
        json!({"peer":device.to_string(),"method":"hub.status","target":null,"generation":1,"expires_unix_s":100}),
    );
    assert_eq!(
        call(&path, &rollback).await["receipt"]["error"]["code"],
        "permission_denied"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    drop(kernel);
    let (_kernel, stop, task) = start(dir.path(), hub, device).await;
    assert_eq!(call(&path, &revoke).await, applied);
    assert!(
        authority
            .authorize(device, "hub.status", None, Some(10), 1)
            .is_err()
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn operator_window_and_invite_audit_rollback_expiry_and_lost_reply() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let (kernel, stop, task) = start(dir.path(), hub, device).await;
    let path = dir.path().join("operator.sock");
    let db = rusqlite::Connection::open(dir.path().join("kernel.db")).unwrap();
    let fail = "CREATE TRIGGER fail_finish BEFORE INSERT ON audit WHEN json_extract(NEW.record,'$.method')='operator.finish' BEGIN SELECT RAISE(ABORT,'injected'); END;";
    db.execute_batch(fail).unwrap();
    let window = req(20, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(call(&path, &window).await["receipt"]["state"], "queued");
    assert_eq!(
        db.query_row("SELECT count(*) FROM proto_operator_windows", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    let secret = "88".repeat(32);
    let invite = req(
        21,
        "enrollment.invite",
        json!({"window_id":window["operation_id"],"expected_device":device.to_string(),"secret_sha256":hex::encode(Sha256::digest(secret.as_bytes())),"ttl_ms":60000,"grants":[]}),
    );
    assert_eq!(
        call(&path, &invite).await["receipt"]["state"],
        "queued",
        "failure receipt audit must preserve admitted observation"
    );
    db.execute_batch("DROP TRIGGER fail_finish").unwrap();
    // Failed window COMMIT must not have published in-memory authority.
    assert_eq!(
        call(&path, &invite).await["receipt"]["error"]["code"],
        "unauthenticated"
    );
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    db.execute_batch(fail).unwrap();
    let mut fresh = invite.clone();
    fresh["operation_id"] = json!(format!("{:032x}", 22));
    assert_eq!(call(&path, &fresh).await["receipt"]["state"], "queued");
    assert_eq!(
        db.query_row("SELECT count(*) FROM proto_invites", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        enroll(&kernel, hub, device, &secret).await["error"]["code"],
        "unauthenticated"
    );
    db.execute_batch("DROP TRIGGER fail_finish").unwrap();
    // Drop the actual first reply, not merely a duplicate completed response.
    let mut lost = tokio::net::UnixStream::connect(&path).await.unwrap();
    let bytes = serde_json::to_vec(&fresh).unwrap();
    lost.write_u32(bytes.len() as u32).await.unwrap();
    lost.write_all(&bytes).await.unwrap();
    lost.shutdown().await.unwrap();
    drop(lost);
    let applied = call(&path, &fresh).await;
    assert_eq!(applied["receipt"]["state"], "applied");
    assert_eq!(call(&path, &fresh).await, applied);
    assert_eq!(enroll(&kernel, hub, device, &secret).await["type"], "reply");
    // Closing/replacing a window cannot be undone by an old terminal retry.
    let close = req(23, "enrollment.window", json!({"duration_ms":0}));
    assert_eq!(call(&path, &close).await["receipt"]["state"], "applied");
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    let short = req(24, "enrollment.window", json!({"duration_ms":10}));
    assert_eq!(call(&path, &short).await["receipt"]["state"], "applied");
    tokio::time::sleep(Duration::from_millis(25)).await;
    let expired = req(
        25,
        "enrollment.invite",
        json!({"window_id":short["operation_id"],"expected_device":device.to_string(),"secret_sha256":"99".repeat(32),"ttl_ms":60000,"grants":[]}),
    );
    assert_eq!(
        call(&path, &expired).await["receipt"]["error"]["code"],
        "unauthenticated"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn operator_local_policy_denies_unprovisioned_peer_even_without_initial_grants() {
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let (_kernel, stop, task) = start(dir.path(), hub, device).await;
    let path = dir.path().join("operator.sock");
    let window = req(30, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    let other = SecretKey::generate().public();
    let invite = req(
        31,
        "enrollment.invite",
        json!({"window_id":window["operation_id"],"expected_device":other.to_string(),"secret_sha256":"aa".repeat(32),"ttl_ms":60000,"grants":[]}),
    );
    assert_eq!(
        call(&path, &invite).await["error"]["code"],
        "permission_denied"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn operator_health_provision_advances_index_without_other_invite_corruption() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let uid = std::fs::metadata(dir.path()).unwrap().uid();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(10))).unwrap();
    let lock = StateLock::acquire(dir.path()).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let mut p = provisioning(uid, device);
    p.grants[0].method = "hub.health".into();
    let service = Arc::new(Service::new(kernel.clone(), uid, p).unwrap());
    let (stop, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server::serve(socket, service, 8, async {
        let _ = rx.await;
    }));
    let path = dir.path().join("operator.sock");
    let window = req(40, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    let secret = "bb".repeat(32);
    let invite = req(
        41,
        "enrollment.invite",
        json!({"window_id":window["operation_id"],"expected_device":device.to_string(),"secret_sha256":hex::encode(Sha256::digest(secret.as_bytes())),"ttl_ms":60000,"grants":[{"peer":device.to_string(),"method":"hub.health","target":null,"generation":1,"expires_unix_s":100}]}),
    );
    assert_eq!(call(&path, &invite).await["receipt"]["state"], "applied");
    let mut other = invite.clone();
    other["operation_id"] = json!(format!("{:032x}", 42));
    other["params"]["secret_sha256"] = json!("cc".repeat(32));
    other["params"]["grants"][0]["generation"] = json!(2);
    assert_eq!(
        call(&path, &other).await["receipt"]["error"]["code"],
        "conflict"
    );
    assert_eq!(enroll(&kernel, hub, device, &secret).await["type"], "reply");
    let provision = req(
        43,
        "grant.provision",
        json!({"peer":device.to_string(),"method":"hub.health","target":null,"generation":2,"expires_unix_s":100}),
    );
    assert_eq!(call(&path, &provision).await["receipt"]["state"], "applied");
    kernel.transport_connected(device).await.unwrap();
    let health = json!({"method":"hub.health","target":null,"timeout_ms":1000,"correlation_id":"33".repeat(16),"params":{"device":device.to_string(),"uptime_ms":0,"queue_depth":0,"available_bytes":1234,"coglets":[]}});
    let response = kernel
        .handle(
            RequestContext {
                peer: device,
                local: hub,
                deadline: tokio::time::Instant::now() + Duration::from_secs(5),
            },
            health,
        )
        .await;
    assert_eq!(response["type"], "reply", "{response}");
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn operator_invite_monotonic_ttl_and_close_have_known_token_controls() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let path = dir.path().join("operator.sock");
    let (kernel, stop, task) = start(dir.path(), hub, device).await;
    let window = req(50, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    let secret = "dd".repeat(32);
    let invite = req(
        51,
        "enrollment.invite",
        json!({"window_id":window["operation_id"],"expected_device":device.to_string(),"secret_sha256":hex::encode(Sha256::digest(secret.as_bytes())),"ttl_ms":1,"grants":[]}),
    );
    assert_eq!(call(&path, &invite).await["receipt"]["state"], "applied");
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        enroll(&kernel, hub, device, &secret).await["error"]["code"],
        "unauthenticated"
    );
    let known = "ee".repeat(32);
    let mut fresh = invite.clone();
    fresh["operation_id"] = json!(format!("{:032x}", 52));
    fresh["params"]["secret_sha256"] = json!(hex::encode(Sha256::digest(known.as_bytes())));
    fresh["params"]["ttl_ms"] = json!(60000);
    assert_eq!(call(&path, &fresh).await["receipt"]["state"], "applied");
    let close = req(53, "enrollment.window", json!({"duration_ms":0}));
    assert_eq!(call(&path, &close).await["receipt"]["state"], "applied");
    assert_eq!(call(&path, &window).await["receipt"]["state"], "applied");
    assert_eq!(
        enroll(&kernel, hub, device, &known).await["error"]["code"],
        "unauthenticated"
    );
    let reopen = req(54, "enrollment.window", json!({"duration_ms":60000}));
    assert_eq!(call(&path, &reopen).await["receipt"]["state"], "applied");
    assert_eq!(
        enroll(&kernel, hub, device, &known).await["error"]["code"],
        "unauthenticated"
    );
    let final_secret = "ff".repeat(32);
    fresh["operation_id"] = json!(format!("{:032x}", 55));
    fresh["params"]["window_id"] = reopen["operation_id"].clone();
    fresh["params"]["secret_sha256"] = json!(hex::encode(Sha256::digest(final_secret.as_bytes())));
    assert_eq!(call(&path, &fresh).await["receipt"]["state"], "applied");
    assert_eq!(
        enroll(&kernel, hub, device, &final_secret).await["type"],
        "reply"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn operator_missing_mapping_denies_socket_admission() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let uid = std::fs::metadata(dir.path()).unwrap().uid();
    let lock = StateLock::acquire(dir.path()).unwrap();
    let kernel = Kernel::open(
        dir.path(),
        SecretKey::generate().public(),
        Arc::new(|| Some(10)),
    )
    .unwrap();
    let service = Arc::new(Service::unprovisioned(kernel.clone()));
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let (stop, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server::serve(socket, service, 8, async {
        let _ = rx.await;
    }));
    let r = req(60, "enrollment.window", json!({"duration_ms":1000}));
    assert_eq!(
        call(&dir.path().join("operator.sock"), &r).await["error"]["code"],
        "permission_denied"
    );
    assert!(kernel.audit().await.unwrap().is_empty());
    let p = provisioning(uid.wrapping_add(1), SecretKey::generate().public());
    assert!(Service::new(kernel, uid, p).is_err());
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
fn provisioning(uid: u32, device: iroh::EndpointId) -> Provisioning {
    serde_json::from_value(json!({"uid":uid,"actor":format!("coglet:{}","11".repeat(32)),"operations":["grant.provision","grant.revoke","enrollment.window","enrollment.invite"],"max_window_ms":300000,"grants":[{"peer":device.to_string(),"method":"hub.status","target":null,"max_expires_unix_s":1000}]})).unwrap()
}
#[tokio::test]
async fn operator_service_socket_grant_configuration_and_lost_reply_retry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        dir.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let lock = StateLock::acquire(dir.path()).unwrap();
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(dir.path()).unwrap().uid();
    let hub = SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(10))).unwrap();
    let device = SecretKey::generate().public();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let secret = kernel
        .invite(device, Duration::from_secs(60), vec![])
        .await
        .unwrap();
    let authority = coglet_proto::authority::Authority::open(dir.path().join("kernel.db")).unwrap();
    let enrolled=kernel.handle(RequestContext {peer:device,local:hub,deadline:tokio::time::Instant::now()+Duration::from_secs(5)},json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":secret}})).await;
    assert_eq!(enrolled["type"], "reply", "{enrolled}");
    let service = Arc::new(Service::new(kernel.clone(), uid, provisioning(uid, device)).unwrap());
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let path = dir.path().join("operator.sock");
    let (stop, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server::serve(socket, service, 8, async {
        let _ = rx.await;
    }));
    let request = req(
        1,
        "grant.provision",
        json!({"peer":device.to_string(),"method":"hub.status","target":null,"generation":1,"expires_unix_s":100}),
    );
    let result = call(&path, &request).await;
    assert_eq!(result["receipt"]["state"], "applied", "{result}");
    authority
        .authorize(device, "hub.status", None, Some(10), 1)
        .unwrap();
    assert_eq!(call(&path, &request).await, result);
    let count = kernel.audit().await.unwrap().len();
    assert_eq!(call(&path, &request).await, result);
    assert_eq!(kernel.audit().await.unwrap().len(), count);
    let forbidden = req(
        2,
        "grant.provision",
        json!({"peer":device.to_string(),"method":"hub.stop","target":null,"generation":1,"expires_unix_s":100}),
    );
    assert_eq!(
        call(&path, &forbidden).await["error"]["code"],
        "permission_denied"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
