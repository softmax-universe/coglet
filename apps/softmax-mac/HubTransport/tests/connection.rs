use coglet_mock::hub::MockHub;
use iroh::SecretKey;
use softmax_hub_link::{connect, endpoint};
use std::time::Duration;

#[tokio::test]
async fn enroll_drop_offline_recover_same_identity() {
    let hub = MockHub::start().await.unwrap();
    let device = endpoint(SecretKey::from_bytes(&[91; 32])).await.unwrap();
    let invite = hub
        .invite(device.id(), Duration::from_secs(60), vec![])
        .await
        .unwrap();
    let c = connect(&device, hub.addr(), Some(invite)).await.unwrap();
    assert!(c.close_reason().is_none());
    c.close(0u32.into(), b"test drop");
    c.closed().await;
    assert!(c.close_reason().is_some());
    let recovered = connect(&device, hub.addr(), None).await.unwrap();
    assert!(recovered.close_reason().is_none());
    hub.kernel().revoke_peer(device.id()).await.unwrap();
    assert!(connect(&device, hub.addr(), None).await.is_err());
    device.close().await;
    hub.shutdown().await;
}
