use super::*;

#[tokio::test]
async fn expected_reset_requires_exact_code_and_frame_boundary() -> Result<()> {
    timeout(Duration::from_secs(20), async {
        for (expected, code, partial, succeeds) in [
            (true, Some(1u32), false, true),
            (true, None, false, false),
            (true, Some(2), false, false),
            (false, Some(1), false, false),
            (true, None, true, false),
        ] {
            let router = Endpoint::builder(presets::Minimal)
                .alpns(vec![b"reset-test".to_vec()])
                .bind()
                .await?;
            let device = Endpoint::bind(presets::Minimal).await?;
            let accept = tokio::spawn(accept_pinned(router.clone(), device.id()));
            let local = device.connect(router.addr(), b"reset-test").await?;
            let remote = accept.await??;
            let (mut send, _recv) = remote.open_bi().await?;
            write_frame(&mut send, b"request").await?;
            let (_send, recv) = local.accept_bi().await?;
            let (output, mut reader) = tokio::io::duplex(1024);
            let job = tokio::spawn(copy_operation(
                100,
                recv,
                Arc::new(Mutex::new(output)),
                expected,
            ));
            assert_eq!(read_frame(&mut reader).await?.unwrap(), b"request");
            if partial {
                send.write_all(&[0, 0]).await?;
            }
            if let Some(code) = code {
                send.reset(code.into())?;
            } else {
                send.finish()?;
            }
            let result = job.await?;
            assert_eq!(
                result.is_ok(),
                succeeds,
                "expected={expected}, code={code:?}, partial={partial}: {result:?}"
            );
            if succeeds {
                assert_eq!(result?, (100, true));
            }
            local.close(0u32.into(), b"done");
            device.close().await;
            router.close().await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn bridge_observes_reset_and_retires_unfinished_send() -> Result<()> {
    timeout(Duration::from_secs(10), async {
        let router = Endpoint::builder(presets::Minimal).alpns(vec![b"reset-bridge".to_vec()]).bind().await?;
        let device = Endpoint::bind(presets::Minimal).await?;
        let accept = tokio::spawn(accept_pinned(router.clone(), device.id()));
        let local = device.connect(router.addr(), b"reset-bridge").await?;
        let remote = accept.await??;
        let (mut input, reader) = tokio::io::duplex(1024);
        let (output, mut replies) = tokio::io::duplex(1024);
        let job = tokio::spawn(bridge(local.clone(), reader, output, Some(100), Some(100)));
        let (mut send, mut recv) = remote.open_bi().await?;
        write_frame(&mut send, b"request").await?;
        assert_eq!(read_frame(&mut replies).await?.unwrap(), b"request");
        let mut control = 100u64.to_be_bytes().to_vec();
        control.push(0);
        control.extend_from_slice(b"metadata");
        write_frame(&mut input, &control).await?;
        assert_eq!(read_frame(&mut recv).await?.unwrap(), b"metadata");
        send.reset(1u32.into())?;
        drop(input);
        job.await??;
        assert!(matches!(recv.read(&mut [0]).await, Err(iroh::endpoint::ReadError::Reset(code)) if code == 1u32.into()));
        assert!(read_frame(&mut replies).await?.is_none());
        local.close(0u32.into(), b"done");
        device.close().await;
        router.close().await;
        Ok::<_, anyhow::Error>(())
    }).await?
}
