//! Bounded raw-byte credit accounting for git transfer. The sink owns quarantine
//! durability; call `consumed` only after accepted chunks have been durably stored.
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
#[error("invalid git transfer")]
pub struct InvalidTransfer;

pub struct Receiver {
    id: String,
    size: u64,
    hash: Vec<u8>,
    digest: Sha256,
    offset: u64,
    consumed: u64,
    credit: u64,
    ended: bool,
}

impl Receiver {
    pub fn new(id: &str, size: u64, hash: &str) -> Result<Self, InvalidTransfer> {
        if !(1..=67108864).contains(&size) || !lower_hex(id, 32) || !lower_hex(hash, 64) {
            return Err(InvalidTransfer);
        }
        Ok(Self {
            id: id.into(),
            size,
            hash: hex::decode(hash).map_err(|_| InvalidTransfer)?,
            digest: Sha256::new(),
            offset: 0,
            consumed: 0,
            credit: 0,
            ended: false,
        })
    }

    /// Cumulative credit, never more than 64KiB ahead of durable consumption.
    pub fn grant(&mut self, bytes: u64) -> Result<(), InvalidTransfer> {
        if self.ended
            || bytes < self.credit
            || bytes > self.size
            || bytes.saturating_sub(self.consumed) > 65536
        {
            return Err(InvalidTransfer);
        }
        self.credit = bytes;
        Ok(())
    }

    pub fn chunk(&mut self, frame: &Value) -> Result<Vec<u8>, InvalidTransfer> {
        if self.ended {
            return Err(InvalidTransfer);
        }
        self.frame("hub-chunk", frame)?;
        let offset = frame["offset"].as_u64().ok_or(InvalidTransfer)?;
        let bytes = hex::decode(frame["hex"].as_str().ok_or(InvalidTransfer)?)
            .map_err(|_| InvalidTransfer)?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(InvalidTransfer)?;
        if offset != self.offset || end > self.credit || end > self.size {
            return Err(InvalidTransfer);
        }
        self.digest.update(&bytes);
        self.offset = end;
        Ok(bytes)
    }

    pub fn consumed(&mut self) {
        self.consumed = self.offset;
    }

    pub fn finish(&mut self, frame: &Value) -> Result<(), InvalidTransfer> {
        self.frame("hub-end", frame)?;
        if self.ended
            || self.offset != self.size
            || self.consumed != self.size
            || self.digest.clone().finalize().as_slice() != self.hash
        {
            return Err(InvalidTransfer);
        }
        self.ended = true;
        Ok(())
    }

    fn frame(&self, rule: &str, frame: &Value) -> Result<(), InvalidTransfer> {
        let value = crate::wire::profile(frame).map_err(|_| InvalidTransfer)?;
        coglet_types::validate(rule, &value).map_err(|_| InvalidTransfer)?;
        if frame["correlation_id"] != self.id {
            return Err(InvalidTransfer);
        }
        Ok(())
    }
}

/// Sender-side state. No raw bytes may be sent before cumulative receiver credit.
/// Each returned frame accounts for its bytes exactly once; a failed write must
/// abort the stream rather than retry the frame through this state machine.
pub struct Sender {
    id: String,
    size: u64,
    credit: u64,
    offset: u64,
    ended: bool,
}

impl Sender {
    pub fn new(id: &str, size: u64) -> Result<Self, InvalidTransfer> {
        if !lower_hex(id, 32) || !(1..=67108864).contains(&size) {
            return Err(InvalidTransfer);
        }
        Ok(Self {
            id: id.into(),
            size,
            credit: 0,
            offset: 0,
            ended: false,
        })
    }

    pub fn credit(&mut self, frame: &Value) -> Result<(), InvalidTransfer> {
        let value = crate::wire::profile(frame).map_err(|_| InvalidTransfer)?;
        coglet_types::validate("hub-credit", &value).map_err(|_| InvalidTransfer)?;
        let credit = frame["bytes"].as_u64().ok_or(InvalidTransfer)?;
        if self.ended
            || frame["correlation_id"] != self.id
            || credit < self.credit
            || credit > self.size
            || credit.saturating_sub(self.offset) > 65536
        {
            return Err(InvalidTransfer);
        }
        self.credit = credit;
        Ok(())
    }

    /// Bytes currently sendable, excluding data already accounted for.
    pub fn available(&self) -> u64 {
        self.credit - self.offset
    }

    pub fn chunk(&mut self, bytes: &[u8]) -> Result<Value, InvalidTransfer> {
        let end = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or(InvalidTransfer)?;
        if self.ended || bytes.is_empty() || bytes.len() > 32768 || end > self.credit {
            return Err(InvalidTransfer);
        }
        let frame = serde_json::json!({"type":"chunk", "correlation_id":self.id,
            "offset":self.offset, "hex":hex::encode(bytes)});
        self.offset = end;
        Ok(frame)
    }

    pub fn finish(&mut self) -> Result<Value, InvalidTransfer> {
        if self.ended || self.offset != self.size {
            return Err(InvalidTransfer);
        }
        self.ended = true;
        Ok(serde_json::json!({"type":"end", "correlation_id":self.id}))
    }
}

/// A bounded quarantine writer. Implementations must return only after the
/// entire slice is durably stored, or fail. Flush to an in-memory buffer or an
/// OS page cache alone is not durability. The operation owner removes the sink
/// on any error, timeout or cancellation and verifies Git before committing refs.
pub trait DurableSink {
    fn append(
        &mut self,
        bytes: &[u8],
    ) -> impl std::future::Future<Output = std::io::Result<()>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error(transparent)]
    Transfer(#[from] InvalidTransfer),
    #[error(transparent)]
    Frame(#[from] crate::wire::FrameError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Sends the raw bundle data phase. Request/metadata must already be sent.
/// The operation owner supplies a deadline/reset guard and checks the terminal
/// reply. This function never emits FIN or claims successful Git admission.
pub async fn send_chunks<W, R, B>(
    send: &mut W,
    recv: &mut R,
    id: &str,
    size: u64,
    source: &mut B,
) -> Result<(), StreamError>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncRead + Unpin,
    B: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut state = Sender::new(id, size)?;
    let mut remaining = size;
    let mut buffer = [0; 32768];
    while remaining > 0 {
        while state.available() == 0 {
            state.credit(&crate::wire::read_frame(recv).await?)?;
        }
        let count = state.available().min(remaining).min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..count]).await?;
        crate::wire::write_frame(send, &state.chunk(&buffer[..count])?).await?;
        remaining -= count as u64;
    }
    crate::wire::write_frame(send, &state.finish()?).await?;
    Ok(())
}

/// Receives into bounded durable quarantine. Caller verifies signed metadata
/// and its latest watermark BEFORE calling (this emits credit immediately), and
/// verifies Git/archive/provenance + rechecks authority/watermark afterward.
/// Replenishes only when a whole <=64KiB window has been durably consumed,
/// avoiding unbounded queued credit frames. FIN/trailing-data checks, deadline,
/// reset-on-abandon and terminal response remain with the operation owner.
pub async fn receive_chunks<W, R, S>(
    send: &mut W,
    recv: &mut R,
    id: &str,
    size: u64,
    hash: &str,
    sink: &mut S,
) -> Result<(), StreamError>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncRead + Unpin,
    S: DurableSink,
{
    let mut state = Receiver::new(id, size, hash)?;
    let mut granted = size.min(65536);
    state.grant(granted)?;
    crate::wire::write_frame(
        send,
        &serde_json::json!({"type":"credit", "correlation_id":id, "bytes":granted}),
    )
    .await?;
    loop {
        let frame = crate::wire::read_frame(recv).await?;
        if frame["type"] == "end" {
            state.finish(&frame)?;
            return Ok(());
        }
        let bytes = state.chunk(&frame)?;
        sink.append(&bytes).await?;
        state.consumed();
        if state.offset == granted && granted < size {
            granted = size.min(granted + 65536);
            state.grant(granted)?;
            crate::wire::write_frame(
                send,
                &serde_json::json!({"type":"credit", "correlation_id":id, "bytes":granted}),
            )
            .await?;
        }
    }
}

fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
