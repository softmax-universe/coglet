//! End-to-end: Python runner -> real driver process -> encrypted local iroh peer.
use std::{process::Command, time::Duration};

use anyhow::{Context, Result};
use iroh::{Endpoint, endpoint::presets};
use tokio::time::timeout;

#[tokio::test]
async fn python_runner_through_driver_binary() -> Result<()> {
    timeout(Duration::from_secs(20), async {
        let alpn = "coglet/hub/0";
        let server = Endpoint::builder(presets::Minimal)
            .alpns(vec![alpn.as_bytes().to_vec()])
            .bind()
            .await?;
        let addr = server.addr();
        let direct = addr
            .ip_addrs()
            .next()
            .context("no direct address")?
            .to_string();
        let id = server.id().to_string();
        let temporary = tempfile::tempdir()?;
        let cases = temporary.path().join("iroh-test");
        std::fs::create_dir(&cases)?;
        std::fs::write(
            cases.join("echo.json"),
            r#"{
            "name":"encrypted-binary-roundtrip",
            "steps":[
                {"send":{"counter":3},"stream":7,"finish":false},
                {"expect":{"counter":3},"within_ms":5000},
                {"send":{"counter":4},"stream":7,"finish":true},
                {"expect":{"counter":4},"within_ms":5000}
            ]
        }"#,
        )?;
        let directory = temporary.path().to_owned();
        let runner = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../run");
        let child = tokio::task::spawn_blocking(move || {
            Command::new("python3")
                .arg(runner)
                .args([
                    "--endpoint",
                    &id,
                    "--alpn",
                    alpn,
                    "--address",
                    &direct,
                    "--driver",
                    env!("CARGO_BIN_EXE_iroh-driver"),
                    "--suite",
                    "iroh-test",
                ])
                .arg("--cases")
                .arg(&directory)
                .arg("--junit")
                .arg(directory.join("result.xml"))
                .output()
        });
        let conn = server.accept().await.context("accept ended")?.await?;
        let (mut send, mut recv) = conn.accept_bi().await?;
        for _ in 0..2 {
            let mut header = [0; 4];
            recv.read_exact(&mut header).await?;
            let size = u32::from_be_bytes(header) as usize;
            assert!(size <= 1024);
            let mut frame = header.to_vec();
            frame.resize(4 + size, 0);
            recv.read_exact(&mut frame[4..]).await?;
            send.write_all(&frame).await?;
        }
        assert!(recv.read_to_end(1024).await?.is_empty());
        send.finish()?;
        let output = child.await??;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("PASS direct.iroh-test"));
        let xml = std::fs::read_to_string(temporary.path().join("result.xml"))?;
        assert!(xml.contains("tests=\"1\""));
        assert!(xml.contains("failures=\"0\""));
        server.close().await;
        Ok::<_, anyhow::Error>(())
    })
    .await?
}
