#![cfg(unix)]
use coglet_hub::operator::socket::OperatorSocket;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

#[tokio::test]
async fn operator_stale_socket_requires_exclusive_state_lock() {
    use coglet_hub::operator::socket::StateLock;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let lock = StateLock::acquire(dir.path()).unwrap();
    let live = OperatorSocket::bind_locked(&lock).await.unwrap();
    assert!(StateLock::acquire(dir.path()).is_err());
    assert!(OperatorSocket::bind_locked(&lock).await.is_err());
    // A process crash leaves the entry but closes the descriptor. Reproduce that
    // kernel state without relying on a timing-sensitive child kill.
    let path = dir.path().join("operator.sock");
    let leftover = dir.path().join("leftover.sock");
    fs::rename(&path, &leftover).unwrap();
    drop(live);
    fs::rename(&leftover, &path).unwrap();
    drop(lock);
    let lock = StateLock::acquire(dir.path()).unwrap();
    let reopened = OperatorSocket::bind_locked(&lock).await.unwrap();
    let client = UnixStream::connect(&path).await.unwrap();
    let (server, _) = reopened.accept().await.unwrap();
    drop((client, server, reopened));
    // An arbitrary regular file and a symlink may never be removed as stale.
    fs::write(&path, b"preserve").unwrap();
    assert!(OperatorSocket::bind_locked(&lock).await.is_err());
    assert_eq!(fs::read(&path).unwrap(), b"preserve");
    fs::remove_file(&path).unwrap();
    let victim = dir.path().join("victim");
    fs::write(&victim, b"victim").unwrap();
    symlink(&victim, &path).unwrap();
    assert!(OperatorSocket::bind_locked(&lock).await.is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"victim");
}

#[tokio::test]
async fn operator_socket_permissions() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = OperatorSocket::bind(dir.path()).unwrap();
    let path = dir.path().join("operator.sock");
    assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert!(OperatorSocket::bind(dir.path()).is_err());
    let client = UnixStream::connect(&path).await.unwrap();
    let (server, peer) = socket.accept().await.unwrap();
    assert_eq!(peer.uid, fs::metadata(dir.path()).unwrap().uid());
    assert!(peer.pid.is_some());
    drop((client, server, socket));
    assert!(!path.exists());
    let victim = dir.path().join("victim");
    fs::write(&victim, b"keep").unwrap();
    symlink(&victim, &path).unwrap();
    assert!(OperatorSocket::bind(dir.path()).is_err());
    assert_eq!(fs::read(victim).unwrap(), b"keep");
}

#[tokio::test]
async fn operator_socket_directory_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(OperatorSocket::bind(dir.path()).is_err());
    let alias_dir = tempfile::tempdir().unwrap();
    symlink(dir.path(), alias_dir.path().join("state")).unwrap();
    assert!(OperatorSocket::bind(&alias_dir.path().join("state")).is_err());
}

#[tokio::test]
async fn operator_socket_frame_bounds() {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(&1048577_u32.to_be_bytes()).await.unwrap();
    assert!(
        coglet_hub::operator::socket::read_request(&mut b)
            .await
            .is_err()
    );
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(&0_u32.to_be_bytes()).await.unwrap();
    assert!(
        coglet_hub::operator::socket::read_request(&mut b)
            .await
            .is_err()
    );
    let (mut a, mut b) = UnixStream::pair().unwrap();
    coglet_hub::operator::socket::write_response(&mut a, &serde_json::json!({"ok":true}))
        .await
        .unwrap();
    let n = b.read_u32().await.unwrap();
    let mut bytes = vec![0; n as usize];
    b.read_exact(&mut bytes).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::json!({"ok":true})
    );
}
