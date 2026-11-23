//! Observe reset codes on actual QUIC streams, not mocked async IO.
use coglet_proto::{
    transport::{ALPN, Role, Session},
    wire::{read_frame, write_frame},
};
use iroh::{Endpoint, endpoint::presets};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn timeout_cancellation_malformed_and_trailing_replies_stop_with_code_one() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let hub = Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let h = hub.clone();
        let accept = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
        let dc = device.connect(hub.addr(), ALPN).await.unwrap();
        let hc = accept.await.unwrap();
        let session = Session::new(dc, device.id(), Role::Device).unwrap();
        for mode in ["timeout", "cancel", "malformed", "trailing"] {
            let request = json!({"method":"hub.enroll","correlation_id":"ef".repeat(16),"target":null,"timeout_ms":if mode == "timeout" {100} else {2000},"params":{"offer":{"versions":["0.1"]},"invite":null}});
            let s = session.clone();
            let task = tokio::spawn(async move { s.request(request).await });
            let (mut send, mut recv) = hc.accept_bi().await.unwrap();
            let request = read_frame(&mut recv).await.unwrap();
            assert_eq!(recv.read(&mut [0]).await.unwrap(), None);
            match mode {
                "cancel" => { task.abort(); },
                "malformed" => { send.write_all(&0u32.to_be_bytes()).await.unwrap(); },
                "trailing" => {
                    write_frame(&mut send, &json!({"type":"error","correlation_id":request["correlation_id"],"error":{"code":"permission_denied","message":"test","retryable":false}})).await.unwrap();
                    send.write_all(&[0]).await.unwrap();
                },
                _ => {},
            }
            assert_eq!(send.stopped().await.unwrap().unwrap().into_inner(), 1, "{mode}");
            if mode == "cancel" { assert!(task.await.unwrap_err().is_cancelled()); }
            else { assert!(task.await.unwrap().is_err()); }
        }
        // Capacity remains usable after every failed caller future.
        let s = session.clone();
        let task = tokio::spawn(async move { s.request(json!({"method":"hub.enroll","correlation_id":"ab".repeat(16),"target":null,"timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":null}})).await });
        let (mut send, mut recv) = hc.accept_bi().await.unwrap();
        let request = read_frame(&mut recv).await.unwrap();
        write_frame(&mut send, &json!({"type":"error","correlation_id":request["correlation_id"],"error":{"code":"permission_denied","message":"usable","retryable":false}})).await.unwrap();
        send.finish().unwrap();
        assert_eq!(task.await.unwrap().unwrap()["error"]["message"], "usable");
        device.close().await; hub.close().await;
    }).await.unwrap();
}
