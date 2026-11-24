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

async fn bridge<R, W>(
    conn: iroh::endpoint::Connection,
    mut input: R,
    output: W,
    incoming_id: Option<u64>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let output = Arc::new(Mutex::new(output));
    let mut operations = JoinSet::new();
    let mut finishes = JoinSet::new();
    let mut sends = HashMap::new();
    let mut used = HashSet::new();
    let mut waiting_incoming = incoming_id;
    // Reserve before reading stdin: an early send must never silently open a
    // different QUIC stream for the reverse-initiated operation.
    used.extend(incoming_id);
    let incoming = timeout(LIMIT, conn.accept_bi());
    tokio::pin!(incoming);
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
                    Some(result) = finishes.join_next(), if !finishes.is_empty() => {
                        result??;
                    }
                    streams = &mut incoming, if waiting_incoming.is_some() => {
                        let (send, recv) = streams.context("incoming operation timeout")??;
                        let id = waiting_incoming.take().context("missing incoming id")?;
                        if operations.len() >= 64 {
                            bail!("too many in-flight operations");
                        }
                        // Publish routing before the request bytes reach stdout.
                        sends.insert(id, send);
                        operations.spawn(copy_replies(recv, output.clone()));
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
        if waiting_incoming == Some(id) {
            bail!("send before reserved incoming stream was accepted");
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = sends.entry(id) {
            if !used.insert(id) || used.len() > 10000 {
                bail!("stream id reused or operation limit exceeded");
            }
            if operations.len() >= 64 {
                bail!("too many in-flight operations");
            }
            let (send, recv) = conn.open_bi().await?;
            entry.insert(send);
            operations.spawn(copy_replies(recv, output.clone()));
        }
        let send = sends.get_mut(&id).context("missing send stream")?;
        timeout(LIMIT, write_frame(send, &frame[9..])).await??;
        if finish {
            if finishes.len() >= 64 {
                bail!("too many unacknowledged operation FINs");
            }
            let mut send = sends.remove(&id).context("missing send stream")?;
            send.finish()?;
            // A remote FIN alone does not prove delivery of our final frames.
            // Keep send custody until transport acknowledgement, without
            // blocking receipt of reverse requests or other operations.
            finishes.spawn(async move {
                if let Some(code) = timeout(LIMIT, send.stopped())
                    .await
                    .context("operation FIN acknowledgement timeout")??
                {
                    bail!("peer stopped operation stream with code {code}");
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }
    if waiting_incoming.is_some() {
        bail!("input EOF before expected incoming operation");
    }
    if !sends.is_empty() {
        bail!("input EOF with unfinished operation streams");
    }
    while let Some(result) = operations.join_next().await {
        result??;
    }
    while let Some(result) = finishes.join_next().await {
        result??;
    }
    Ok(())
}

async fn copy_replies<W: AsyncWrite + Unpin + Send + 'static>(
    mut recv: iroh::endpoint::RecvStream,
    output: Arc<Mutex<W>>,
) -> Result<()> {
    timeout(LIMIT, async {
        while let Some(reply) = read_frame(&mut recv).await? {
            write_frame(&mut *output.lock().await, &reply).await?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("operation timeout")?
}

async fn accept_pinned(endpoint: Endpoint, peer: EndpointId) -> Result<iroh::endpoint::Connection> {
    let connection = timeout(LIMIT, async {
        endpoint
            .accept()
            .await
            .context("listener closed")?
            .await
            .context("accept handshake")
    })
    .await
    .context("accept timeout")??;
    if connection.remote_id() != peer {
        connection.close(1u32.into(), b"unexpected peer");
        bail!("accepted connection does not match pinned peer");
    }
    Ok(connection)
}

async fn run() -> Result<()> {
    let mut endpoint_id = None;
    let mut address = None;
    let mut alpn = None;
    let mut mode = "dial".to_owned();
    let mut incoming_id = None;
    let mut key_file = std::env::var("COGLET_FIXTURE_IROH_KEY_FILE").ok();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().context("every option requires a value")?;
        match arg.as_str() {
            "--endpoint" => endpoint_id = Some(value.parse::<EndpointId>()?),
            "--address" => address = Some(value.parse::<SocketAddr>()?),
            "--alpn" => alpn = Some(value),
            "--key-file" => key_file = Some(value),
            "--mode" => mode = value,
            "--incoming-stream" => incoming_id = Some(value.parse::<u64>()?),
            _ => bail!("unknown option {arg}"),
        }
    }
    if !matches!(mode.as_str(), "dial" | "accept") {
        bail!("--mode must be dial or accept");
    }
    let peer = endpoint_id.context("--endpoint required (pinned peer in either mode)")?;
    let alpn = alpn.context("--alpn required")?;
    let mut remote = EndpointAddr::new(peer);
    let builder = if mode == "accept" {
        let address = address.context("accept mode requires --address for local bind")?;
        if !address.ip().is_loopback() {
            bail!("conformance listener requires a loopback address");
        }
        Endpoint::builder(presets::Minimal)
            .alpns(vec![alpn.as_bytes().to_vec()])
            .bind_addr(address)?
    } else if let Some(address) = address {
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
    let connection = if mode == "accept" {
        // Never mix local discovery with the framed stdout protocol. Fixture
        // launchers parse this single stderr line before dialing the listener.
        let addr = endpoint.addr();
        let bound = addr
            .ip_addrs()
            .find(|a| a.ip().is_loopback())
            .context("listener has no loopback address")?;
        eprintln!("COGLET_IROH_LISTENING {} {bound}", endpoint.id());
        accept_pinned(endpoint.clone(), peer).await
    } else {
        timeout(LIMIT, endpoint.connect(remote, alpn.as_bytes()))
            .await
            .context("connect timeout")?
            .context("connect handshake")
    };
    let result = match connection {
        Ok(conn) => {
            let result = bridge(
                conn.clone(),
                tokio::io::stdin(),
                tokio::io::stdout(),
                incoming_id,
            )
            .await;
            conn.close(0u32.into(), b"driver finished");
            result
        }
        Err(error) => Err(error),
    };
    endpoint.close().await;
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    run().await
}

#[cfg(test)]
mod reverse_tests;

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
            let driver = tokio::spawn(bridge(conn.clone(), reader, output, None));
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
