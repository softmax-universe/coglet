use coglet_hub::Kernel;
use coglet_proto::{
    authority::Grant,
    transport::{OperationHandler, RequestContext},
};
use iroh::SecretKey;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn health_uses_accepted_local_elapsed_time_and_exact_connection_lifetimes() {
    let dir = tempfile::tempdir().unwrap();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let now = Arc::new(AtomicU64::new(0));
    let elapsed = now.clone();
    let kernel = Kernel::open_with_elapsed(
        dir.path(),
        hub,
        Arc::new(|| Some(1)),
        Arc::new(move || Duration::from_millis(elapsed.load(Ordering::SeqCst))),
    )
    .unwrap();
    let first = kernel.transport_connected(device).await.unwrap();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = kernel
        .invite(
            device,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.health".into(),
                target: None,
                expires: 1000,
                generation: 7,
            }],
        )
        .await
        .unwrap();
    let ctx = || RequestContext {
        peer: device,
        local: hub,
        deadline: tokio::time::Instant::now() + Duration::from_secs(5),
    };
    let enroll = json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}});
    assert_eq!(kernel.handle(ctx(), enroll).await["type"], "reply");
    let health = json!({"method":"hub.health","target":null,"timeout_ms":1000,"correlation_id":"22".repeat(16),"params":{"device":device.to_string(),"uptime_ms":999999,"queue_depth":0,"available_bytes":1234,"coglets":[]}});
    assert_eq!(kernel.handle(ctx(), health.clone()).await["type"], "reply");
    assert!(
        kernel
            .invite(
                device,
                Duration::from_secs(60),
                vec![Grant {
                    method: "hub.health".into(),
                    target: None,
                    expires: 1000,
                    generation: 8
                }]
            )
            .await
            .is_err()
    );
    assert_eq!(kernel.handle(ctx(), health.clone()).await["type"], "reply");
    assert_eq!(
        kernel.status().await.unwrap()["devices"][0]["state"],
        "connected"
    );
    now.store(30000, Ordering::SeqCst);
    assert_eq!(
        kernel.status().await.unwrap()["devices"][0]["state"],
        "connected"
    );
    now.store(30001, Ordering::SeqCst);
    assert_eq!(
        kernel.status().await.unwrap()["devices"][0]["state"],
        "degraded"
    );
    let mut forged = health.clone();
    forged["params"]["device"] = json!(hub.to_string());
    assert_eq!(kernel.handle(ctx(), forged).await["type"], "error");
    assert_eq!(
        kernel.status().await.unwrap()["devices"][0]["state"],
        "degraded"
    );
    let second = kernel.transport_connected(device).await.unwrap();
    kernel.transport_disconnected(device, first).await.unwrap();
    assert_eq!(kernel.handle(ctx(), health).await["type"], "reply");
    assert_eq!(
        kernel.status().await.unwrap()["devices"][0]["state"],
        "connected"
    );
    kernel.transport_disconnected(device, second).await.unwrap();
    assert_eq!(
        kernel.status().await.unwrap()["devices"][0]["state"],
        "disconnected"
    );
    drop(kernel);
    let reopened = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let status = reopened.status().await.unwrap();
    assert_eq!(status["devices"][0]["state"], "disconnected");
    assert!(status["devices"][0]["last_health_age_ms"].is_null());
}
