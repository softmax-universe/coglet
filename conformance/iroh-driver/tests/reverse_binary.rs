//! Exercise listener discovery/pinning and incoming stream routing in the actual
//! binary, and separately route reverse fetch through Python's registry path.
use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, endpoint::presets};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

const ALPN: &[u8] = b"coglet/conformance/black-box";

async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let size = reader.read_u32().await? as usize;
    anyhow::ensure!((1..=1024).contains(&size));
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}
async fn write<W: AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[tokio::test]
async fn binary_accepts_device_dial_and_opens_reverse_stream() -> Result<()> {
    timeout(Duration::from_secs(20), async {
        let device = Endpoint::bind(presets::Minimal).await?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_iroh-driver"))
            .args([
                "--mode",
                "accept",
                "--endpoint",
                &device.id().to_string(),
                "--address",
                "127.0.0.1:0",
                "--alpn",
                std::str::from_utf8(ALPN)?,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stderr = BufReader::new(child.stderr.take().context("stderr")?);
        let mut discovery = String::new();
        stderr.read_line(&mut discovery).await?;
        let fields: Vec<_> = discovery.split_whitespace().collect();
        anyhow::ensure!(
            fields.len() == 3 && fields[0] == "COGLET_IROH_LISTENING",
            "{discovery}"
        );
        let id: EndpointId = fields[1].parse()?;
        let addr = EndpointAddr::new(id).with_ip_addr(fields[2].parse()?);
        let conn = device.connect(addr, ALPN).await?;
        let mut stdin = child.stdin.take().context("stdin")?;
        let mut stdout = child.stdout.take().context("stdout")?;
        let mut control = u64::MAX.to_be_bytes().to_vec();
        control.push(1);
        control.extend_from_slice(b"reverse");
        write(&mut stdin, &control).await?;
        let (mut send, mut recv) = conn.accept_bi().await?;
        assert_eq!(read(&mut recv).await?, b"reverse");
        assert!(recv.read_to_end(1024).await?.is_empty());
        write(&mut send, b"response").await?;
        send.finish()?;
        assert_eq!(read(&mut stdout).await?, b"response");
        drop(stdin);
        let result = child.wait().await?;
        assert!(result.success(), "driver {result}");
        device.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn python_registry_drives_reverse_fetch_on_existing_connection() -> Result<()> {
    timeout(Duration::from_secs(20), async {
        let router = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let addr = router.addr();
        let direct = addr.ip_addrs().next().context("direct address")?;
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("reverse-test"))?;
        std::fs::write(
            root.path().join("reverse-test/case.json"),
            r#"{
            "name":"opaque-reverse-fetch", "steps":[
                {"send":1}, {"expect":2,"within_ms":5000},
                {"send":7},
                {"expect":3,"within_ms":5000},
                {"send":4,"stream":100,"finish":false},
                {"expect":5,"within_ms":5000},
                {"send":6,"stream":100,"finish":true}
            ]}"#,
        )?;
        let registry = root.path().join("registry.toml");
        std::fs::write(
            &registry,
            format!(
                r#"
[[implementations]]
name = "reverse-fixture"
suites = ["reverse-test"]
transport = "iroh"
endpoint = "{}"
alpn = "{}"
address = "{direct}"
driver = {:?}
incoming_stream = 100
"#,
                router.id(),
                std::str::from_utf8(ALPN)?,
                env!("CARGO_BIN_EXE_iroh-driver")
            ),
        )?;
        let mut runner = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../run"))
            .arg("--registry")
            .arg(registry)
            .arg("--cases")
            .arg(root.path())
            .arg("--junit")
            .arg(root.path().join("result.xml"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let connection = tokio::select! {
            incoming = router.accept() => incoming.context("accept")?,
            status = runner.wait() => {
                let mut errors = String::new();
                runner.stderr.take().context("stderr")?.read_to_string(&mut errors).await?;
                anyhow::bail!("runner ended before connection: {:?}: {errors}", status?);
            }
        };
        let conn = connection.await?;
        let (mut send, mut recv) = conn.accept_bi().await?;
        assert_eq!(read(&mut recv).await?, [1]);
        assert!(recv.read_to_end(1024).await?.is_empty());
        write(&mut send, &[2]).await?;
        send.finish()?;
        // Wait for the unchanged reply assertion before publishing a reverse
        // stream: QUIC imposes no ordering between separate operation streams.
        let (mut barrier_send, mut barrier_recv) = conn.accept_bi().await?;
        assert_eq!(read(&mut barrier_recv).await?, [7]);
        assert!(barrier_recv.read_to_end(1024).await?.is_empty());
        barrier_send.finish()?;
        let (mut send, mut recv) = conn.open_bi().await?;
        write(&mut send, &[3]).await?;
        assert_eq!(read(&mut recv).await?, [4]);

        write(&mut send, &[5]).await?;
        assert_eq!(read(&mut recv).await?, [6]);
        assert!(recv.read_to_end(1024).await?.is_empty());
        send.finish()?;
        let status = runner.wait().await?;
        let mut errors = String::new();
        runner
            .stderr
            .take()
            .context("runner stderr")?
            .read_to_string(&mut errors)
            .await?;
        assert!(status.success(), "{status}: {errors}");
        let xml = std::fs::read_to_string(root.path().join("result.xml"))?;
        assert!(xml.contains("tests=\"1\""));
        assert!(xml.contains("failures=\"0\""));
        assert!(xml.contains("skipped=\"0\""));
        router.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn binary_rejects_invalid_mode_and_nonlocal_listener() -> Result<()> {
    for args in [
        vec!["--mode", "unknown"],
        vec![
            "--mode",
            "accept",
            "--endpoint",
            "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
            "--address",
            "0.0.0.0:0",
            "--alpn",
            "test",
        ],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_iroh-driver"))
            .args(args)
            .kill_on_drop(true)
            .output()
            .await?;
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        let error = String::from_utf8(result.stderr)?;
        assert!(
            error.contains("--mode must be") || error.contains("loopback address"),
            "{error}"
        );
    }
    // Verify fixture seed parsing stays compatible with the discovery identity.
    assert_eq!(
        SecretKey::from_bytes(&[2; 32]).public().to_string(),
        "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394"
    );
    Ok(())
}
