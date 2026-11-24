//! Real encrypted connection tests; payloads deliberately are not CBOR so the
//! driver cannot accidentally reconstruct or reinterpret protocol messages.
use super::*;

const ALPN: &[u8] = b"coglet/conformance/reverse-test";

async fn control<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: u64,
    finish: bool,
    payload: &[u8],
) -> Result<()> {
    let mut bytes = id.to_be_bytes().to_vec();
    bytes.push(u8::from(finish));
    bytes.extend_from_slice(payload);
    write_frame(writer, &bytes).await
}

#[tokio::test]
async fn device_dials_pinned_listener_then_receives_reverse_operation() -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let router = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let device = Endpoint::bind(presets::Minimal).await?;
        let accepting = tokio::spawn(accept_pinned(router.clone(), device.id()));
        let connection = device.connect(router.addr(), ALPN).await?;
        let accepted = accepting.await??;
        assert_eq!(accepted.remote_id(), device.id());
        let (mut input, reader) = tokio::io::duplex(1024);
        let (output, mut replies) = tokio::io::duplex(1024);
        let driver = tokio::spawn(bridge(accepted.clone(), reader, output, None));
        control(&mut input, u64::MAX, true, b"reverse request").await?;
        let (mut send, mut recv) = connection.accept_bi().await?;
        assert_eq!(read_frame(&mut recv).await?.unwrap(), b"reverse request");
        assert!(read_frame(&mut recv).await?.is_none());
        write_frame(&mut send, b"reverse reply").await?;
        send.finish()?;
        assert_eq!(read_frame(&mut replies).await?.unwrap(), b"reverse reply");
        drop(input);
        driver.await??;
        accepted.close(0u32.into(), b"test complete");
        device.close().await;
        router.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn listener_rejects_authenticated_but_unpinned_identity() -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let router = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let device = Endpoint::bind(presets::Minimal).await?;
        let other = Endpoint::bind(presets::Minimal).await?;
        let accepting = tokio::spawn(accept_pinned(router.clone(), other.id()));
        let connection = device.connect(router.addr(), ALPN).await?;
        let error = accepting.await?.unwrap_err();
        assert!(error.to_string().contains("pinned peer"), "{error}");
        connection.closed().await;
        device.close().await;
        router.close().await;
        other.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn incoming_fetch_maps_to_100_and_preserves_credit_chunk_fin() -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let router = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let device = Endpoint::bind(presets::Minimal).await?;
        let accepting = tokio::spawn(accept_pinned(router.clone(), device.id()));
        let local = device.connect(router.addr(), ALPN).await?;
        let remote = accepting.await??;
        let (mut input, reader) = tokio::io::duplex(1024);
        let (output, mut replies) = tokio::io::duplex(1024);
        let driver = tokio::spawn(bridge(local.clone(), reader, output, Some(100)));
        let (mut send, mut recv) = remote.open_bi().await?;
        write_frame(&mut send, b"fetch").await?;
        assert_eq!(read_frame(&mut replies).await?.unwrap(), b"fetch");
        control(&mut input, 100, false, b"metadata").await?;
        assert_eq!(read_frame(&mut recv).await?.unwrap(), b"metadata");
        write_frame(&mut send, b"credit").await?;
        assert_eq!(read_frame(&mut replies).await?.unwrap(), b"credit");
        // Concurrent locally-opened operation remains independent of stream 100.
        control(&mut input, 1 << 63, true, b"enroll").await?;
        let (mut other_send, mut other_recv) = remote.accept_bi().await?;
        assert_eq!(read_frame(&mut other_recv).await?.unwrap(), b"enroll");
        assert!(read_frame(&mut other_recv).await?.is_none());
        write_frame(&mut other_send, b"enrolled").await?;
        other_send.finish()?;
        assert_eq!(read_frame(&mut replies).await?.unwrap(), b"enrolled");
        for frame in [b"chunk".as_slice(), b"end".as_slice()] {
            control(&mut input, 100, false, frame).await?;
            assert_eq!(read_frame(&mut recv).await?.unwrap(), frame);
        }
        control(&mut input, 100, true, b"reply").await?;
        assert_eq!(read_frame(&mut recv).await?.unwrap(), b"reply");
        assert!(read_frame(&mut recv).await?.is_none());
        send.finish()?;
        drop(input);
        driver.await??;
        local.close(0u32.into(), b"test complete");
        device.close().await;
        router.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn reserved_incoming_id_cannot_silently_open_outgoing_stream() -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let router = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let device = Endpoint::bind(presets::Minimal).await?;
        let accepting = tokio::spawn(accept_pinned(router.clone(), device.id()));
        let local = device.connect(router.addr(), ALPN).await?;
        let remote = accepting.await??;
        let (mut input, reader) = tokio::io::duplex(1024);
        let (output, _replies) = tokio::io::duplex(1024);
        let driver = tokio::spawn(bridge(local.clone(), reader, output, Some(100)));
        control(&mut input, 100, true, b"premature").await?;
        let error = driver.await?.unwrap_err();
        assert!(
            error.to_string().contains("reserved incoming stream"),
            "{error}"
        );
        // The failure must not have allocated a stream on the wire.
        assert!(
            timeout(Duration::from_millis(50), remote.accept_bi())
                .await
                .is_err()
        );
        local.close(0u32.into(), b"test complete");
        device.close().await;
        router.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}
