use super::*;
#[tokio::test]
async fn disconnect_is_not_lost_when_request_admission_is_saturated() {
    let dir = tempfile::tempdir().unwrap();
    let hub = iroh::SecretKey::generate().public();
    let peer = iroh::SecretKey::generate().public();
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let token = k.transport_connected(peer).await.unwrap();
    let permits = k.blocking.acquire_many(8).await.unwrap();
    k.transport_disconnected(peer, token).await.unwrap();
    drop(permits);
    assert_eq!(
        k.status().await.unwrap()["devices"][0]["state"],
        "disconnected"
    );
}
