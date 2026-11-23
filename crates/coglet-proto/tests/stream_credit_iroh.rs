//! Raw Git data-phase component tests, not complete Git push/fetch admission.
use coglet_proto::{
    transfer::{DurableSink, receive_chunks, send_chunks},
    transport::ALPN,
};
use iroh::{Endpoint, endpoint::presets};
use sha2::{Digest, Sha256};
use std::{io, time::Duration};

#[derive(Default)]
struct Sink(Vec<u8>);
impl DurableSink for Sink {
    async fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

struct GatedSink {
    entered: tokio::sync::mpsc::Sender<()>,
    release: std::sync::Arc<tokio::sync::Semaphore>,
    bytes: Vec<u8>,
}
impl DurableSink for GatedSink {
    async fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.entered.send(()).await.unwrap();
        self.release.acquire().await.unwrap().forget();
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

#[tokio::test]
async fn real_iroh_no_new_credit_until_entire_previous_window_is_durable() {
    use coglet_proto::wire::{read_frame, write_frame};
    use serde_json::json;
    let listener = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let dialer = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let listening = listener.clone();
    let accepted = tokio::spawn(async move { listening.accept().await.unwrap().await.unwrap() });
    let client = dialer.connect(listener.addr(), ALPN).await.unwrap();
    let server = accepted.await.unwrap();
    let bytes = vec![7u8; 100_000];
    let hash = hex::encode(Sha256::digest(&bytes));
    let (mut send, mut recv) = client.open_bi().await.unwrap();
    send.write_all(&[42]).await.unwrap();
    let (mut response, mut request) = server.accept_bi().await.unwrap();
    use tokio::io::AsyncReadExt;
    assert_eq!(request.read_u8().await.unwrap(), 42);
    let (entered_tx, mut entered) = tokio::sync::mpsc::channel(2);
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let receiver = tokio::spawn(async move {
        let mut sink = GatedSink {
            entered: entered_tx,
            release: gate,
            bytes: vec![],
        };
        receive_chunks(
            &mut response,
            &mut request,
            &"ab".repeat(16),
            100_000,
            &hash,
            &mut sink,
        )
        .await
        .unwrap();
        assert!(request.read(&mut [0]).await.unwrap().is_none());
        sink.bytes
    });
    let id = "ab".repeat(16);
    assert_eq!(read_frame(&mut recv).await.unwrap()["bytes"], 65536);
    for offset in [0, 32768] {
        write_frame(&mut send, &json!({"type":"chunk","correlation_id":id,"offset":offset,"hex":hex::encode(&bytes[..32768])})).await.unwrap();
    }
    entered.recv().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), read_frame(&mut recv))
            .await
            .is_err()
    );
    release.add_permits(1);
    entered.recv().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), read_frame(&mut recv))
            .await
            .is_err()
    );
    release.add_permits(1);
    assert_eq!(read_frame(&mut recv).await.unwrap()["bytes"], 100000);
    release.add_permits(2);
    for (offset, data) in [(65536, &bytes[65536..98304]), (98304, &bytes[98304..])] {
        write_frame(
            &mut send,
            &json!({"type":"chunk","correlation_id":id,"offset":offset,"hex":hex::encode(data)}),
        )
        .await
        .unwrap();
    }
    write_frame(&mut send, &json!({"type":"end","correlation_id":id}))
        .await
        .unwrap();
    send.finish().unwrap();
    // Sink notifications have bounded capacity; consume both final appends.
    entered.recv().await.unwrap();
    entered.recv().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .unwrap()
            .unwrap(),
        bytes
    );
    dialer.close().await;
    listener.close().await;
}

#[tokio::test]
async fn failed_durable_sink_never_replenishes_credit() {
    use coglet_proto::wire::{read_frame, write_frame};
    use serde_json::json;
    struct FailedSink;
    impl DurableSink for FailedSink {
        async fn append(&mut self, _: &[u8]) -> io::Result<()> {
            Err(io::Error::other("injected fsync failure"))
        }
    }
    let (mut client, mut server) = tokio::io::duplex(200_000);
    let receiver = tokio::spawn(async move {
        let (mut recv, mut send) = tokio::io::split(&mut server);
        receive_chunks(
            &mut send,
            &mut recv,
            &"ab".repeat(16),
            100_000,
            &"00".repeat(32),
            &mut FailedSink,
        )
        .await
    });
    assert_eq!(read_frame(&mut client).await.unwrap()["bytes"], 65536);
    write_frame(
        &mut client,
        &json!({"type":"chunk", "correlation_id":"ab".repeat(16),
        "offset":0, "hex":hex::encode([7;32768])}),
    )
    .await
    .unwrap();
    assert!(matches!(
        receiver.await.unwrap(),
        Err(coglet_proto::transfer::StreamError::Io(_))
    ));
    use tokio::io::AsyncReadExt;
    assert_eq!(client.read(&mut [0]).await.unwrap(), 0);
}

#[tokio::test]
async fn sender_does_not_read_source_before_credit_and_truncation_emits_no_end() {
    use coglet_proto::wire::{read_frame, write_frame};
    use serde_json::json;
    use std::{
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll},
    };
    struct Source(Arc<AtomicUsize>);
    impl tokio::io::AsyncRead for Source {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Ok(())) // immediate EOF
        }
    }
    let polls = Arc::new(AtomicUsize::new(0));
    let source_polls = polls.clone();
    let (mut client, mut server) = tokio::io::duplex(1024);
    let (started, ready) = tokio::sync::oneshot::channel();
    let sending = tokio::spawn(async move {
        let (mut recv, mut send) = tokio::io::split(&mut client);
        started.send(()).unwrap();
        send_chunks(
            &mut send,
            &mut recv,
            &"ab".repeat(16),
            1,
            &mut Source(source_polls),
        )
        .await
    });
    ready.await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    write_frame(
        &mut server,
        &json!({"type":"credit","correlation_id":"ab".repeat(16),"bytes":1}),
    )
    .await
    .unwrap();
    assert!(matches!(
        sending.await.unwrap(),
        Err(coglet_proto::transfer::StreamError::Io(_))
    ));
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert!(read_frame(&mut server).await.is_err());
}

#[tokio::test]
async fn real_iroh_multiwindow_transfer_preserves_exact_bytes() {
    let listener = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let dialer = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let listening = listener.clone();
    let accepted = tokio::spawn(async move { listening.accept().await.unwrap().await.unwrap() });
    let client = dialer.connect(listener.addr(), ALPN).await.unwrap();
    let server = accepted.await.unwrap();
    let bytes: Vec<u8> = (0..150_000).map(|i| (i % 251) as u8).collect();
    let hash = hex::encode(Sha256::digest(&bytes));
    let sent = bytes.clone();
    let live_client = client.clone();
    let sending = tokio::spawn(async move {
        let (mut send, mut recv) = client.open_bi().await.unwrap();
        // Make stream visible before waiting for credit; a real operation has
        // already written its request/metadata at this point.
        send.write_all(&[42]).await.unwrap();
        send_chunks(
            &mut send,
            &mut recv,
            &"ab".repeat(16),
            sent.len() as u64,
            &mut sent.as_slice(),
        )
        .await
        .unwrap();
        send.finish().unwrap();
    });
    let (mut send, mut recv) = server.accept_bi().await.unwrap();
    use tokio::io::AsyncReadExt;
    assert_eq!(recv.read_u8().await.unwrap(), 42);
    let mut sink = Sink::default();
    tokio::time::timeout(
        Duration::from_secs(5),
        receive_chunks(
            &mut send,
            &mut recv,
            &"ab".repeat(16),
            bytes.len() as u64,
            &hash,
            &mut sink,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(sink.0, bytes);
    assert!(recv.read(&mut [0]).await.unwrap().is_none());
    sending.await.unwrap();
    drop(live_client);
    dialer.close().await;
    listener.close().await;
}
