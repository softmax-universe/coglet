#![cfg(feature = "test-controls")]
use coglet_hub::Kernel;
use coglet_proto::authority::Grant;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn kernel_fixed_invite_uses_health_policy_and_closes_window() {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Kernel::open(
        dir.path(),
        iroh::SecretKey::generate().public(),
        Arc::new(|| Some(1)),
    )
    .unwrap();
    let peer = iroh::SecretKey::generate().public();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let grant = Grant {
        method: "hub.health".into(),
        target: None,
        expires: 100,
        generation: 1,
    };
    kernel
        .test_invite(
            peer,
            "11".repeat(32),
            Duration::from_secs(60),
            vec![grant.clone()],
        )
        .await
        .unwrap();
    let mut conflicting = grant;
    conflicting.generation = 2;
    assert!(
        kernel
            .test_invite(
                peer,
                "22".repeat(32),
                Duration::from_secs(60),
                vec![conflicting]
            )
            .await
            .is_err()
    );
    kernel.test_close_window().await.unwrap();
    assert!(
        kernel
            .test_invite(peer, "33".repeat(32), Duration::from_secs(60), vec![])
            .await
            .is_err()
    );
}
