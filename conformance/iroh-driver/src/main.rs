//! Framed stdin/stdout <-> iroh. No protocol messages are interpreted here.
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, EndpointAddr, EndpointId, endpoint::presets};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::Mutex,
    task::JoinSet,
    time::timeout,
};

const MAX_FRAME: usize = 1_048_576;
const LIMIT: Duration = Duration::from_secs(300);

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    read_frame_limit(reader, MAX_FRAME).await
}

async fn read_frame_limit<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    // EOF between operations is normal, partial header/body is not.
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..]).await?;
    let size = u32::from_be_bytes(header) as usize;
    if !(1..=maximum).contains(&size) {
        bail!("frame size {size} outside 1..={maximum}");
    }
    let mut frame = vec![0; size];
    reader.read_exact(&mut frame).await?;
    Ok(Some(frame))
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) -> Result<()> {
    if !(1..=MAX_FRAME).contains(&frame.len()) {
        bail!("invalid frame size");
    }
    writer
        .write_all(&(frame.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(frame).await?;
    writer.flush().await?;
    Ok(())
}

async fn bridge<R, W>(conn: iroh::endpoint::Connection, mut input: R, output: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let output = Arc::new(Mutex::new(output));
    let mut operations = JoinSet::new();
    let mut sends = HashMap::new();
    let mut used = HashSet::new();
    loop {
        // Keep the read future alive while reaping completed operations: read_exact
        // is not cancellation-safe after consuming a partial frame.
        let frame = {
            let next = read_frame_limit(&mut input, MAX_FRAME + 9);
            tokio::pin!(next);
            loop {
                tokio::select! {
                    frame = &mut next => break frame?,
                    Some(result) = operations.join_next(), if !operations.is_empty() => {
                        result??;
                    }
                }
            }
        };
        let Some(frame) = frame else { break };
        if frame.len() < 10 || frame[8] > 1 {
            bail!("invalid driver control prefix");
        }
        let id = u64::from_be_bytes(frame[..8].try_into()?);
        let finish = frame[8] == 1;
        if let std::collections::hash_map::Entry::Vacant(entry) = sends.entry(id) {
            if !used.insert(id) || used.len() > 10000 {
                bail!("stream id reused or operation limit exceeded");
            }
            if operations.len() >= 64 {
                bail!("too many in-flight operations");
            }
            let (send, mut recv) = conn.open_bi().await?;
            entry.insert(send);
            let output = output.clone();
            operations.spawn(async move {
                timeout(LIMIT, async {
                    while let Some(reply) = read_frame(&mut recv).await? {
                        write_frame(&mut *output.lock().await, &reply).await?;
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await
                .context("operation timeout")?
            });
        }
        let send = sends.get_mut(&id).context("missing send stream")?;
        timeout(LIMIT, write_frame(send, &frame[9..])).await??;
        if finish {
            sends.remove(&id).context("missing send stream")?.finish()?;
        }
    }
    if !sends.is_empty() {
        bail!("input EOF with unfinished operation streams");
    }
    while let Some(result) = operations.join_next().await {
        result??;
    }
    Ok(())
}

async fn run() -> Result<()> {
    let mut endpoint_id = None;
    let mut address = None;
    let mut alpn = None;
    let mut key_file = std::env::var("COGLET_FIXTURE_IROH_KEY_FILE").ok();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().context("every option requires a value")?;
        match arg.as_str() {
            "--endpoint" => endpoint_id = Some(value.parse::<EndpointId>()?),
            "--address" => address = Some(value.parse::<SocketAddr>()?),
            "--alpn" => alpn = Some(value),
            "--key-file" => key_file = Some(value),
            _ => bail!("unknown option {arg}"),
        }
    }
    let mut remote = EndpointAddr::new(endpoint_id.context("--endpoint required")?);
    let builder = if let Some(address) = address {
        remote = remote.with_ip_addr(address);
        Endpoint::builder(presets::Minimal)
    } else {
        Endpoint::builder(presets::N0)
    };
    let builder = if let Some(path) = key_file {
        let key = std::fs::read_to_string(path).context("read fixture key file")?;
        builder.secret_key(key.trim().parse().context("parse fixture secret key")?)
    } else {
        builder
    };
    let endpoint = builder.bind().await?;
    let alpn = alpn.context("--alpn required")?;
    let conn = timeout(LIMIT, endpoint.connect(remote, alpn.as_bytes())).await??;
    let result = bridge(conn.clone(), tokio::io::stdin(), tokio::io::stdout()).await;
    conn.close(0u32.into(), b"driver finished");
    endpoint.close().await;
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_partial_and_oversized_frames() -> Result<()> {
        for bytes in [&[0u8, 0][..], &[0, 16, 0, 1][..], &[0, 0, 0, 2, 1][..]] {
            assert!(read_frame(&mut &bytes[..]).await.is_err());
        }
        assert!(read_frame(&mut &b""[..]).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn real_iroh_two_operations_multiple_replies() -> Result<()> {
        timeout(Duration::from_secs(15), async {
            let alpn = b"coglet/conformance/self-test";
            let server = Endpoint::builder(presets::Minimal)
                .alpns(vec![alpn.to_vec()])
                .bind()
                .await?;
            let client = Endpoint::bind(presets::Minimal).await?;
            let remote = server.addr();
            let server_task = tokio::spawn(async move {
                let incoming = server.accept().await.context("accept ended")?;
                let conn = incoming.await?;
                for _ in 0..2 {
                    let (mut send, mut recv) = conn.accept_bi().await?;
                    let request = read_frame(&mut recv).await?.context("missing request")?;
                    assert!(read_frame(&mut recv).await?.is_none());
                    write_frame(&mut send, &request).await?;
                    write_frame(&mut send, &request).await?;
                    send.finish()?;
                }
                conn.closed().await;
                server.close().await;
                Ok::<_, anyhow::Error>(())
            });
            let conn = client.connect(remote, alpn).await?;
            let (mut writer, reader) = tokio::io::duplex(1024);
            let (output, mut replies) = tokio::io::duplex(1024);
            let driver = tokio::spawn(bridge(conn.clone(), reader, output));
            for (id, frame) in [b"first".as_slice(), b"second".as_slice()]
                .into_iter()
                .enumerate()
            {
                let mut control = (id as u64).to_be_bytes().to_vec();
                control.push(1);
                control.extend_from_slice(frame);
                write_frame(&mut writer, &control).await?;
                assert_eq!(read_frame(&mut replies).await?.unwrap(), frame);
                assert_eq!(read_frame(&mut replies).await?.unwrap(), frame);
            }
            drop(writer);
            driver.await??;
            conn.close(0u32.into(), b"test complete");
            client.close().await;
            server_task.await??;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }
}
