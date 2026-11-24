use coglet_proto::transport::{ALPN, OperationHandler, RequestContext, Role, Session};
use iroh::{
    Endpoint,
    endpoint::{Connection, ReadError, presets},
};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

struct Never;
impl OperationHandler for Never {
    fn handle<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async { panic!("unclassified stream was dispatched") })
    }
}
async fn connect(hub: &Endpoint, device: &Endpoint) -> (Connection, tokio::task::JoinHandle<()>) {
    let h = hub.clone();
    let accept = tokio::spawn(async move {
        let connection = h.accept().await.unwrap().await.unwrap();
        let _ = Session::new(connection, h.id(), Role::Hub)
            .unwrap()
            .serve(Arc::new(Never))
            .await;
    });
    (device.connect(hub.addr(), ALPN).await.unwrap(), accept)
}
async fn assert_reset_one(recv: &mut iroh::endpoint::RecvStream) {
    let error = tokio::time::timeout(Duration::from_secs(4), recv.read(&mut [0; 1]))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error, ReadError::Reset(code) if code == 1u32.into()),
        "{error:?}"
    );
}

#[tokio::test]
async fn unclassified_budget_is_shared_across_connections_and_expires_before_enrollment() {
    tokio::time::timeout(Duration::from_secs(12), async {
        let hub = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let (first, task1) = connect(&hub, &device).await;
        let (second, task2) = connect(&hub, &device).await;
        let (mut s1, mut r1) = first.open_bi().await.unwrap();
        let (mut s2, mut r2) = second.open_bi().await.unwrap();
        // A partial prefix makes each stream visible without completing its frame.
        s1.write_all(&[0]).await.unwrap();
        s2.write_all(&[0]).await.unwrap();
        // Ensure the two first streams have reached admission, but not their 2s deadline.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (mut s3, mut r3) = second.open_bi().await.unwrap();
        s3.write_all(&[0]).await.unwrap();
        tokio::time::timeout(Duration::from_millis(500), assert_reset_one(&mut r3))
            .await
            .expect("third unclassified stream was not immediately rejected");
        assert_reset_one(&mut r1).await;
        assert_reset_one(&mut r2).await;
        // Both slots were released: a fresh malformed stream is independently reset.
        let (mut send, mut recv) = first.open_bi().await.unwrap();
        send.write_all(&[0, 0, 0, 0]).await.unwrap();
        assert_reset_one(&mut recv).await;
        device.close().await;
        hub.close().await;
        task1.abort();
        task2.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn malformed_frames_reset_code_one_without_poisoning_other_connection() {
    tokio::time::timeout(Duration::from_secs(12), async {
        let hub = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let (first, task1) = connect(&hub, &device).await;
        let (second, task2) = connect(&hub, &device).await;
        for (i, bytes) in [
            vec![0, 0, 0, 0],
            vec![0, 16, 0, 1],
            vec![0, 0],
            vec![0, 0, 0, 2, 0],
            vec![0, 0, 0, 1, 255],
        ]
        .into_iter()
        .enumerate()
        {
            let connection = if i % 2 == 0 { &first } else { &second };
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            send.write_all(&bytes).await.unwrap();
            send.finish().unwrap();
            assert_reset_one(&mut recv).await;
        }
        device.close().await;
        hub.close().await;
        task1.abort();
        task2.abort();
    })
    .await
    .unwrap();
}
