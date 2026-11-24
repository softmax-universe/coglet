use coglet_proto::{quarantine::BundleSink, transfer::DurableSink};
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "coglet-sink-{}",
        iroh::SecretKey::generate().public()
    ));
    fs::create_dir(&path).unwrap();
    path
}
#[tokio::test]
async fn real_iroh_multiwindow_data_phase_fsyncs_bounded_bundle_file() {
    use coglet_proto::{
        transfer::{receive_chunks, send_chunks},
        transport::ALPN,
    };
    use iroh::{Endpoint, endpoint::presets};
    use sha2::{Digest, Sha256};
    let root = root();
    let listener = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let dialer = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let endpoint = listener.clone();
    let accepted = tokio::spawn(async move { endpoint.accept().await.unwrap().await.unwrap() });
    let client = dialer.connect(listener.addr(), ALPN).await.unwrap();
    let server = accepted.await.unwrap();
    let bytes: Vec<u8> = (0..150000).map(|n| (n % 251) as u8).collect();
    let hash = hex::encode(Sha256::digest(&bytes));
    let source = bytes.clone();
    let connection = client.clone();
    let sender = tokio::spawn(async move {
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        // QUIC exposes a new bidi stream only after the first outbound byte.
        send.write_all(&[42]).await.unwrap();
        let mut source = source.as_slice();
        send_chunks(&mut send, &mut recv, &"ab".repeat(16), 150000, &mut source)
            .await
            .unwrap();
        send.finish().unwrap();
    });
    let (mut send, mut recv) = server.accept_bi().await.unwrap();
    use tokio::io::AsyncReadExt;
    assert_eq!(recv.read_u8().await.unwrap(), 42);
    let mut sink = BundleSink::create(&root, 150000).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        receive_chunks(
            &mut send,
            &mut recv,
            &"ab".repeat(16),
            150000,
            &hash,
            &mut sink,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(recv.read(&mut [0]).await.unwrap().is_none());
    sender.await.unwrap();
    let directory = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    assert_eq!(fs::read(directory.join("bundle")).unwrap(), bytes);
    drop(sink);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir(root).unwrap();
    dialer.close().await;
    listener.close().await;
}

#[tokio::test]
async fn durable_bundle_sink_bounds_before_write_and_cleans_up_on_drop() {
    let root = root();
    let mut sink = BundleSink::create(&root, 5).await.unwrap();
    sink.append(b"abc").await.unwrap();
    let dir = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    let bundle = dir.join("bundle");
    assert_eq!(fs::read(&bundle).unwrap(), b"abc");
    assert!(sink.append(b"def").await.is_err());
    assert_eq!(fs::read(&bundle).unwrap(), b"abc");
    sink.append(b"de").await.unwrap();
    assert_eq!(fs::read(&bundle).unwrap(), b"abcde");
    drop(sink);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    assert!(BundleSink::create(&root, 0).await.is_err());
    assert!(BundleSink::create(&root, 67108865).await.is_err());
    fs::remove_dir(root).unwrap();
}
